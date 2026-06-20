// CamillaDSP - A flexible tool for processing audio
// Copyright (C) 2026 Henrik Enquist
//
// This file is part of CamillaDSP.
//
// CamillaDSP is free software; you can redistribute it and/or modify it
// under the terms of either:
//
// a) the GNU General Public License version 3,
//    or
// b) the Mozilla Public License Version 2.0.
//
// You should have received copies of the GNU General Public License and the
// Mozilla Public License along with this program. If not, see
// <https://www.gnu.org/licenses/> and <https://www.mozilla.org/MPL/2.0/>.

use crate::PrcFmt;
use crate::Res;
use crate::audiochunk::AudioChunk;
use crate::config;
use crate::config::BinarySampleFormat;
use crate::processors::Processor;
use crate::utils::conversions::chunk_to_buffer_rawbytes_borrowed;
use crate::utils::wavtools::write_wav_header;
use crossbeam_channel::{Sender, bounded};
use ringbuf::HeapCons;
use ringbuf::{HeapProd, HeapRb, traits::*};
use std::fs::File;
use std::io::Write;
use std::thread;

// Minimum number of chunks to store in the ring buffer
const MIN_CHUNKS: usize = 4;

pub struct FileWriter {
    name: String,
    config: config::FileWriterParameters,
    chunksize: usize,
    producer: HeapProd<PrcFmt>,
    tx_notify: Sender<()>,
    warned: bool,
}

struct WriterThread {
    samplerate: usize,
    channels: usize,
    samples_per_chunk: usize,
    sample_format: BinarySampleFormat,
    filename: String,
    consumer: HeapCons<PrcFmt>,
    rx_notify: crossbeam_channel::Receiver<()>,
    wav_header: bool,
    chunk: AudioChunk,
    bytes: Vec<u8>,
}

impl FileWriter {
    /// Creates a FileWriter processor from a config struct.
    pub fn from_config(
        name: &str,
        config: config::FileWriterParameters,
        samplerate: usize,
        chunksize: usize,
    ) -> Self {
        debug!(
            "Creating FileWriter processor '{}', channels: {}, filename: {}, format: {}, wav_header: {}",
            name,
            config.channels,
            config.filename,
            config.format,
            config.wav_header()
        );
        let channels = config.channels;
        let sample_format = config.format;
        let wav_header = config.wav_header();
        let filename = config.filename.clone();
        let samples_per_chunk = (chunksize * channels).max(1);
        let bytes_per_chunk = samples_per_chunk * sample_format.bytes_per_sample();
        let ring_size = channels * samplerate.max(MIN_CHUNKS * chunksize * channels);
        let ringbuffer = HeapRb::<PrcFmt>::new(ring_size);
        let (producer, consumer) = ringbuffer.split();
        let (tx_notify, rx_notify) = bounded::<()>(2);
        let proc_name = name.to_string();
        if let Err(err) = thread::Builder::new()
            .name(format!("FileWriter-{proc_name}"))
            .spawn(move || {
                let waveforms = (0..channels).map(|_| vec![0.0; chunksize]).collect();
                let chunk = AudioChunk::new(waveforms, 0.0, 0.0, chunksize, chunksize);
                let bytes = vec![0_u8; bytes_per_chunk];
                let writer = WriterThread {
                    samplerate,
                    channels,
                    samples_per_chunk,
                    sample_format,
                    filename,
                    consumer,
                    rx_notify,
                    wav_header,
                    chunk,
                    bytes,
                };
                if let Err(err) = writer.run() {
                    error!("FileWriter processor '{}' writer error: {}", proc_name, err);
                }
            })
        {
            error!(
                "FileWriter processor '{}' failed to spawn writer thread: {}",
                name, err
            );
        }
        FileWriter {
            name: name.to_string(),
            config,
            chunksize,
            producer,
            tx_notify,
            warned: false,
        }
    }
}

impl Processor for FileWriter {
    fn name(&self) -> &str {
        &self.name
    }

    /// Copy the input AudioChunk into the ring buffer.
    fn process_chunk(&mut self, input: &mut AudioChunk) -> Res<()> {
        if input.valid_frames < self.chunksize {
            // Silently drop last partial chunk.
            return Ok(());
        }

        if self.producer.vacant_len() < input.valid_frames * self.config.channels {
            if !self.warned {
                warn!(
                    "FileWriter processor '{}' buffer overrun, dropping chunks",
                    self.name
                );
                self.warned = true;
            }
            return Ok(());
        }
        for channel in 0..self.config.channels {
            let slice = &input.waveforms[channel][..input.valid_frames];
            let _ = self.producer.push_slice(slice);
        }
        let _ = self.tx_notify.try_send(());
        self.warned = false;
        Ok(())
    }

    fn update_parameters(&mut self, config: config::Processor) {
        if let config::Processor::FileWriter {
            parameters: config, ..
        } = config
        {
            if config != self.config {
                panic!("FileWriter does not support parameter change.");
            }
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

impl WriterThread {
    // Consumes `self` and runs the write loop to completion.
    fn run(mut self) -> Result<(), std::io::Error> {
        let mut file = File::create(&self.filename)?;
        if self.wav_header {
            write_wav_header(
                &mut file,
                self.channels,
                self.sample_format,
                self.samplerate,
            )?;
        }
        while let Ok(()) = self.rx_notify.recv() {
            self.drain_and_write(&mut file)?;
        }
        self.drain_and_write(&mut file)?;
        file.flush()?;
        debug!("FileWriter processor '{}' writer done", self.filename);
        Ok(())
    }

    fn drain_and_write(&mut self, file: &mut File) -> Result<(), std::io::Error> {
        while self.consumer.occupied_len() >= self.samples_per_chunk {
            for ch in 0..self.channels {
                self.consumer.pop_slice(&mut self.chunk.waveforms[ch]);
            }
            let (valid_bytes, _) = chunk_to_buffer_rawbytes_borrowed(
                &self.chunk,
                &mut self.bytes,
                &self.sample_format,
            );
            file.write_all(&self.bytes[..valid_bytes])?;
        }
        Ok(())
    }
}

/// Validate the file writer config.
pub fn validate_file_writer(config: &config::FileWriterParameters) -> Res<()> {
    if config.channels == 0 {
        return Err(config::ConfigError::new(
            "FileWriter processor channels must be greater than zero.",
        )
        .into());
    }
    if config.filename.is_empty() {
        return Err(
            config::ConfigError::new("FileWriter processor filename must not be empty.").into(),
        );
    }
    if config.wav_header() && config.format == BinarySampleFormat::S24_4_RJ_LE {
        return Err(config::ConfigError::new(
            "Wav files do not support the S24_4_RJ_LE sample format.",
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PrcFmt;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::Duration;

    static FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_test_filename() -> String {
        let n = FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!(
                "camilladsp_file_writer_test_{}_{n}.raw",
                std::process::id()
            ))
            .to_string_lossy()
            .into_owned()
    }

    /// Drop the `FileWriter`, which drops the `notify` sender so the writer
    /// thread drains its ring buffer and exits, then sleep briefly to let the
    /// detached thread flush the file before the test reads it. The writer
    /// thread is detached, so we cannot join it.
    fn shutdown(fw: FileWriter) {
        drop(fw);
        thread::sleep(Duration::from_millis(100));
    }

    fn f32_config(filename: String) -> config::FileWriterParameters {
        config::FileWriterParameters {
            channels: 2,
            filename,
            format: BinarySampleFormat::F32_LE,
            wav_header: Some(false),
        }
    }

    fn stereo_chunk(left: [f64; 2], right: [f64; 2], valid_frames: usize) -> AudioChunk {
        let all = [left[0], left[1], right[0], right[1]];
        AudioChunk::new(
            vec![
                vec![left[0] as PrcFmt, left[1] as PrcFmt],
                vec![right[0] as PrcFmt, right[1] as PrcFmt],
            ],
            all.iter().copied().fold(f64::NEG_INFINITY, f64::max) as PrcFmt,
            all.iter().copied().fold(f64::INFINITY, f64::min) as PrcFmt,
            2,
            valid_frames,
        )
    }

    fn mono_chunk(samples: [f64; 2], valid_frames: usize) -> AudioChunk {
        AudioChunk::new(
            vec![vec![samples[0] as PrcFmt, samples[1] as PrcFmt]],
            samples[0].max(samples[1]) as PrcFmt,
            samples[0].min(samples[1]) as PrcFmt,
            2,
            valid_frames,
        )
    }

    #[test]
    fn validate_rejects_right_justified_wav() {
        let config = config::FileWriterParameters {
            channels: 2,
            filename: "test.wav".to_string(),
            format: BinarySampleFormat::S24_4_RJ_LE,
            wav_header: Some(true),
        };
        assert!(validate_file_writer(&config).is_err());
    }

    #[test]
    fn writes_three_interleaved_raw_chunks_without_modifying_chunks() {
        let filename = unique_test_filename();
        let mut fw = FileWriter::from_config("test", f32_config(filename.clone()), 48_000, 2);

        let mut c1 = stereo_chunk([0.25, -0.5], [0.75, -1.0], 2);
        let mut c2 = stereo_chunk([0.125, -0.25], [0.5, -0.75], 2);
        // c3 has only 1 valid frame, less than the chunksize of 2. Partial
        // chunks are silently dropped by `process_chunk`, so c3 is never
        // written to the file. It is still passed through to verify the
        // processor does not mutate the input chunk.
        let mut c3 = stereo_chunk([1.0, -0.875], [0.0, 0.375], 1);

        fw.process_chunk(&mut c1).unwrap();
        fw.process_chunk(&mut c2).unwrap();
        fw.process_chunk(&mut c3).unwrap();
        shutdown(fw);

        assert_eq!(c1.waveforms[0], vec![0.25 as PrcFmt, -0.5 as PrcFmt]);
        assert_eq!(c1.waveforms[1], vec![0.75 as PrcFmt, -1.0 as PrcFmt]);
        assert_eq!(c2.waveforms[0], vec![0.125 as PrcFmt, -0.25 as PrcFmt]);
        assert_eq!(c2.waveforms[1], vec![0.5 as PrcFmt, -0.75 as PrcFmt]);
        assert_eq!(c3.waveforms[0], vec![1.0 as PrcFmt, -0.875 as PrcFmt]);
        assert_eq!(c3.waveforms[1], vec![0.0 as PrcFmt, 0.375 as PrcFmt]);

        let data = fs::read(&filename).unwrap();
        let mut expected = Vec::new();
        // Only c1 and c2 are written; c3 is dropped (partial chunk).
        for &value in &[
            0.25f32, 0.75f32, -0.5f32, -1.0f32, 0.125f32, 0.5f32, -0.25f32, -0.75f32,
        ] {
            expected.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(data, expected);
        let _ = fs::remove_file(filename);
    }

    #[test]
    fn accepts_zero_chunksize() {
        let filename = unique_test_filename();
        let mut fw = FileWriter::from_config("test", f32_config(filename.clone()), 48_000, 0);
        let mut chunk = AudioChunk::new(vec![vec![], vec![]], 0.0, 0.0, 0, 0);

        fw.process_chunk(&mut chunk).unwrap();
        shutdown(fw);

        assert!(fs::read(&filename).unwrap().is_empty());
        let _ = fs::remove_file(filename);
    }

    #[test]
    fn drops_chunk_when_ring_buffer_is_full() {
        let filename = unique_test_filename();
        let mut fw = FileWriter::from_config("test", f32_config(filename.clone()), 48_000, 2);
        let ringbuffer = HeapRb::<PrcFmt>::new(1);
        let (mut producer, _consumer) = ringbuffer.split();
        assert!(producer.try_push(0.0).is_ok());
        fw.producer = producer;

        let mut chunk = stereo_chunk([1.0, 2.0], [3.0, 4.0], 2);
        fw.process_chunk(&mut chunk).unwrap();

        assert_eq!(fw.producer.occupied_len(), 1);
        drop(fw);
        let _ = fs::remove_file(filename);
    }

    #[test]
    fn update_parameters_panics_on_changed_config() {
        let filename = unique_test_filename();
        let config = f32_config(filename.clone());
        let mut config2 = f32_config(filename.clone());
        config2.channels = config.channels + 1;

        let mut fw = FileWriter::from_config("test", config.clone(), 48_000, 2);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fw.update_parameters(config::Processor::FileWriter {
                description: None,
                parameters: config2,
            });
        }));
        assert!(result.is_err());

        drop(fw);
        let _ = fs::remove_file(filename);
    }

    #[test]
    fn writes_wav_header_and_samples() {
        let filename = unique_test_filename();
        let config = config::FileWriterParameters {
            channels: 1,
            filename: filename.clone(),
            format: BinarySampleFormat::S16_LE,
            wav_header: Some(true),
        };
        let mut fw = FileWriter::from_config("test", config, 48_000, 2);
        let mut chunk = mono_chunk([0.5, -0.5], 2);

        fw.process_chunk(&mut chunk).unwrap();
        shutdown(fw);

        let data = fs::read(&filename).unwrap();
        assert!(data.starts_with(b"RIFF"));
        assert!(data.len() > 4 + 24 + 8);
        let _ = fs::remove_file(filename);
    }
}
