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

use crate::CamillaFloat;
use crate::Res;
use crate::audiochunk::AudioChunk;
use crate::config;
use crate::config::BinarySampleFormat;
use crate::processors::Processor;
use crate::utils::conversions::chunk_to_buffer_rawbytes_borrowed;
use crossbeam_channel::{Sender, bounded};
use ringbuf::HeapCons;
use ringbuf::{HeapProd, HeapRb, traits::*};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::thread;

// Minimum number of chunks to store in the ring buffer
const MIN_CHUNKS: usize = 4;

pub struct FileWriter {
    name: String,
    config: config::FileWriterParameters,
    process_channels: Vec<usize>,
    chunksize: usize,
    producer: HeapProd<CamillaFloat>,
    tx_notify: Sender<()>,
    warned: bool,
}

struct WriterThread {
    channels: usize,
    samples_per_chunk: usize,
    sample_format: BinarySampleFormat,
    filename: String,
    consumer: HeapCons<CamillaFloat>,
    rx_notify: crossbeam_channel::Receiver<()>,
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
            "Creating FileWriter processor '{}', channels: {}, process_channels: {:?}, filename: {}, format: {}",
            name,
            config.channels,
            config.process_channels(),
            config.filename,
            config.format
        );
        let input_channels = config.channels;
        let mut process_channels = config.process_channels();
        if process_channels.is_empty() {
            for n in 0..input_channels {
                process_channels.push(n);
            }
        }
        let write_channels = process_channels.len();
        let sample_format = config.format;
        let filename = config.filename.clone();
        let samples_per_chunk = chunksize * write_channels;
        let bytes_per_chunk = samples_per_chunk * sample_format.bytes_per_sample();
        let ring_size = write_channels * samplerate.max(MIN_CHUNKS * chunksize * write_channels);
        let ringbuffer = HeapRb::<CamillaFloat>::new(ring_size);
        let (producer, consumer) = ringbuffer.split();
        let (tx_notify, rx_notify) = bounded::<()>(2);
        let proc_name = name.to_string();
        if let Err(err) = thread::Builder::new()
            .name(format!("FileWriter-{proc_name}"))
            .spawn(move || {
                let waveforms = (0..write_channels).map(|_| vec![0.0; chunksize]).collect();
                let chunk = AudioChunk::new(waveforms, 0.0, 0.0, chunksize, chunksize);
                let bytes = vec![0_u8; bytes_per_chunk];
                let writer = WriterThread {
                    channels: write_channels,
                    samples_per_chunk,
                    sample_format,
                    filename,
                    consumer,
                    rx_notify,
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
            process_channels,
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

        if self.producer.vacant_len() < input.valid_frames * self.process_channels.len() {
            if !self.warned {
                warn!(
                    "FileWriter processor '{}' buffer overrun, dropping chunks",
                    self.name
                );
                self.warned = true;
            }
            return Ok(());
        }
        for channel in self.process_channels.iter() {
            let slice = &input.waveforms[*channel][..input.valid_frames];
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
    // Consume `self` and run the write loop to completion.
    fn run(mut self) -> Result<(), std::io::Error> {
        let mut file: Option<File> = None;
        while let Ok(()) = self.rx_notify.recv() {
            self.drain_and_write(&mut file)?;
        }
        self.drain_and_write(&mut file)?;
        if let Some(mut file) = file {
            file.flush()?;
        }
        debug!("FileWriter processor '{}' writer done", self.filename);
        Ok(())
    }

    /// Open unique numbered output file, starting after the highest existing one.
    fn open_file(&mut self) -> Result<File, std::io::Error> {
        let mut counter = highest_existing_counter(&self.filename).map_or(0, |n| n + 1);
        loop {
            let candidate = numbered_filename(&self.filename, counter);
            match OpenOptions::new()
                .write(true)
                // atomic
                .create_new(true)
                .open(&candidate)
            {
                Ok(file) => {
                    self.filename = candidate;
                    return Ok(file);
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    counter += 1;
                }
                Err(err) => return Err(err),
            }
        }
    }

    fn drain_and_write(&mut self, file: &mut Option<File>) -> Result<(), std::io::Error> {
        while self.consumer.occupied_len() >= self.samples_per_chunk {
            for ch in 0..self.channels {
                self.consumer.pop_slice(&mut self.chunk.waveforms[ch]);
            }
            let (valid_bytes, _) = chunk_to_buffer_rawbytes_borrowed(
                &self.chunk,
                &mut self.bytes,
                &self.sample_format,
            );
            if file.is_none() {
                *file = Some(self.open_file()?);
            }
            file.as_mut()
                .unwrap()
                .write_all(&self.bytes[..valid_bytes])?;
        }
        Ok(())
    }
}

fn numbered_filename(filename: &str, counter: u64) -> String {
    format!("{filename}.{counter:03}")
}

fn highest_existing_counter(filename: &str) -> Option<u64> {
    let path = Path::new(filename);
    let name = path.file_name()?.to_str()?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let prefix = format!("{name}.");
    std::fs::read_dir(parent)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            let rest = name.strip_prefix(&prefix)?;
            (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
                .then(|| rest.parse::<u64>().ok())?
        })
        .max()
}

/// Validate the file writer config.
pub fn validate_file_writer(config: &config::FileWriterParameters) -> Res<()> {
    if config.channels == 0 {
        return Err(config::ConfigError::new(
            "FileWriter processor channels must be greater than zero.",
        )
        .into());
    }
    for ch in config.process_channels().iter() {
        if *ch >= config.channels {
            let msg = format!(
                "Invalid channel to process: {}, max is: {}.",
                *ch,
                config.channels - 1
            );
            return Err(config::ConfigError::new(&msg).into());
        }
    }
    if config.filename.is_empty() {
        return Err(
            config::ConfigError::new("FileWriter processor filename must not be empty.").into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CamillaFloat;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::Duration;

    static FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_test_filename() -> String {
        let n = FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!(
                "camilladsp_file_writer_test_{}_{n}",
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
            process_channels: None,
            filename,
            format: BinarySampleFormat::F32_LE,
        }
    }

    fn stereo_chunk(left: [f64; 2], right: [f64; 2], valid_frames: usize) -> AudioChunk {
        let all = [left[0], left[1], right[0], right[1]];
        AudioChunk::new(
            vec![
                vec![left[0] as CamillaFloat, left[1] as CamillaFloat],
                vec![right[0] as CamillaFloat, right[1] as CamillaFloat],
            ],
            all.iter().copied().fold(f64::NEG_INFINITY, f64::max) as CamillaFloat,
            all.iter().copied().fold(f64::INFINITY, f64::min) as CamillaFloat,
            2,
            valid_frames,
        )
    }

    #[test]
    fn validate_rejects_invalid_process_channel() {
        let mut config = f32_config("test".to_string());
        config.process_channels = Some(vec![2]);

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

        assert_eq!(
            c1.waveforms[0],
            vec![0.25 as CamillaFloat, -0.5 as CamillaFloat]
        );
        assert_eq!(
            c1.waveforms[1],
            vec![0.75 as CamillaFloat, -1.0 as CamillaFloat]
        );
        assert_eq!(
            c2.waveforms[0],
            vec![0.125 as CamillaFloat, -0.25 as CamillaFloat]
        );
        assert_eq!(
            c2.waveforms[1],
            vec![0.5 as CamillaFloat, -0.75 as CamillaFloat]
        );
        assert_eq!(
            c3.waveforms[0],
            vec![1.0 as CamillaFloat, -0.875 as CamillaFloat]
        );
        assert_eq!(
            c3.waveforms[1],
            vec![0.0 as CamillaFloat, 0.375 as CamillaFloat]
        );

        let data = fs::read(numbered_filename(&filename, 0)).unwrap();
        let mut expected = Vec::new();
        // Only c1 and c2 are written; c3 is dropped (partial chunk).
        for &value in &[
            0.25f32, 0.75f32, -0.5f32, -1.0f32, 0.125f32, 0.5f32, -0.25f32, -0.75f32,
        ] {
            expected.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(data, expected);
        let _ = fs::remove_file(numbered_filename(&filename, 0));
    }

    #[test]
    fn writes_only_process_channels_in_configured_order() {
        let filename = unique_test_filename();
        let mut config = f32_config(filename.clone());
        config.process_channels = Some(vec![1, 0]);
        let mut fw = FileWriter::from_config("test", config, 48_000, 2);

        let mut chunk = stereo_chunk([0.25, -0.5], [0.75, -1.0], 2);

        fw.process_chunk(&mut chunk).unwrap();
        shutdown(fw);

        let data = fs::read(numbered_filename(&filename, 0)).unwrap();
        let mut expected = Vec::new();
        for &value in &[0.75f32, 0.25f32, -1.0f32, -0.5f32] {
            expected.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(data, expected);
        let _ = fs::remove_file(numbered_filename(&filename, 0));
    }

    #[test]
    fn drops_chunk_when_ring_buffer_is_full() {
        let filename = unique_test_filename();
        let mut fw = FileWriter::from_config("test", f32_config(filename.clone()), 48_000, 2);
        let ringbuffer = HeapRb::<CamillaFloat>::new(1);
        let (mut producer, _consumer) = ringbuffer.split();
        assert!(producer.try_push(0.0).is_ok());
        fw.producer = producer;

        let mut chunk = stereo_chunk([1.0, 2.0], [3.0, 4.0], 2);
        fw.process_chunk(&mut chunk).unwrap();

        assert_eq!(fw.producer.occupied_len(), 1);
        drop(fw);
        let _ = fs::remove_file(numbered_filename(&filename, 0));
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
        let _ = fs::remove_file(numbered_filename(&filename, 0));
    }

    /// Write one stereo chunk through a fresh writer.
    fn write_one_chunk(filename: &str) {
        let mut fw = FileWriter::from_config("test", f32_config(filename.to_string()), 48_000, 2);
        let mut chunk = stereo_chunk([0.25, -0.5], [0.75, -1.0], 2);
        fw.process_chunk(&mut chunk).unwrap();
        shutdown(fw);
    }

    fn expected_bytes() -> Vec<u8> {
        let mut expected = Vec::new();
        for &value in &[0.25f32, 0.75f32, -0.5f32, -1.0f32] {
            expected.extend_from_slice(&value.to_le_bytes());
        }
        expected
    }

    #[test]
    fn claims_next_free_number() {
        // The counter is zero-padded to three digits.
        assert_eq!(numbered_filename("capture", 42), "capture.042");
        // An existing file at 000 blocks the first name.
        let filename = unique_test_filename();
        fs::write(numbered_filename(&filename, 0), b"occupied").unwrap();
        write_one_chunk(&filename);
        assert_eq!(
            fs::read(numbered_filename(&filename, 0)).unwrap(),
            b"occupied"
        );
        assert_eq!(
            fs::read(numbered_filename(&filename, 1)).unwrap(),
            expected_bytes()
        );
        fs::remove_file(numbered_filename(&filename, 0)).unwrap();
        fs::remove_file(numbered_filename(&filename, 1)).unwrap();

        // Gaps are skipped: with 000 and 002 present, the next is 003.
        let filename = unique_test_filename();
        fs::write(numbered_filename(&filename, 0), b"old0").unwrap();
        fs::write(numbered_filename(&filename, 2), b"old2").unwrap();
        write_one_chunk(&filename);
        assert_eq!(fs::read(numbered_filename(&filename, 0)).unwrap(), b"old0");
        assert_eq!(fs::read(numbered_filename(&filename, 2)).unwrap(), b"old2");
        assert!(!Path::new(&numbered_filename(&filename, 1)).exists());
        assert_eq!(
            fs::read(numbered_filename(&filename, 3)).unwrap(),
            expected_bytes()
        );
        for n in [0, 2, 3] {
            fs::remove_file(numbered_filename(&filename, n)).unwrap();
        }

        // A 4-digit name is still counted, so the next is 1001.
        let filename = unique_test_filename();
        fs::write(numbered_filename(&filename, 1000), b"old").unwrap();
        write_one_chunk(&filename);
        assert_eq!(
            fs::read(numbered_filename(&filename, 1000)).unwrap(),
            b"old"
        );
        assert_eq!(
            fs::read(numbered_filename(&filename, 1001)).unwrap(),
            expected_bytes()
        );
        fs::remove_file(numbered_filename(&filename, 1000)).unwrap();
        fs::remove_file(numbered_filename(&filename, 1001)).unwrap();
    }
}
