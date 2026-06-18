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

use crate::Res;
use crate::StatusMessage;
use crate::audiochunk::AudioChunk;
use crate::audiodevice::AudioMessage;
use crate::config;
use crate::config::BinarySampleFormat;
use crate::file_backend::device::AudioWriter;
use crate::processors::Processor;
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use std::fs::File;
use std::io::Write;
use std::thread::{self, JoinHandle};

pub struct FileWriter {
    name: String,
    config: config::FileWriterParameters,
    samplerate: usize,
    chunksize: usize,
    consumer: ConsumerHandle,
    handles: Vec<JoinHandle<()>>,
    /// Set when the consumer has stopped and future chunks should be dropped silently.
    playback_done: bool,
}

struct ConsumerHandle {
    channel: Sender<AudioMessage>,
    status_channel: Receiver<StatusMessage>,
    handle: Option<JoinHandle<()>>,
}

impl ConsumerHandle {
    fn new(
        channel: Sender<AudioMessage>,
        status_channel: Receiver<StatusMessage>,
        handle: Option<JoinHandle<()>>,
    ) -> Self {
        Self {
            channel,
            status_channel,
            handle,
        }
    }
}

// Audio thread functions. The audio pipeline owns `FileWriter` and only pushes complete chunks.
impl FileWriter {
    /// Creates a FileWriter processor from a config struct.
    pub fn from_config(
        name: &str,
        config: config::FileWriterParameters,
        samplerate: usize,
        chunksize: usize,
    ) -> Self {
        debug!(
            "Creating FileWriter processor '{}', channels: {}, filename: {}, format: {}, wav_header: {}, buffer_seconds: {}",
            name,
            config.channels,
            config.filename,
            config.format,
            config.wav_header(),
            config.buffer_seconds()
        );
        let consumer = Self::start_consumer(name, &config, samplerate, chunksize);
        FileWriter {
            name: name.to_string(),
            config,
            samplerate,
            chunksize,
            consumer,
            handles: Vec::new(),
            playback_done: false,
        }
    }

    fn start_consumer(
        name: &str,
        config: &config::FileWriterParameters,
        samplerate: usize,
        chunksize: usize,
    ) -> ConsumerHandle {
        let (tx_audio, rx_audio) = bounded::<AudioMessage>(1);
        let (tx_status, rx_status) = unbounded::<StatusMessage>();
        let name_str = name.to_string();
        let config = config.clone();

        let handle = thread::Builder::new()
            .name(format!("FileWriter-{name}"))
            .spawn(move || {
                if let Err(err) = (|| -> std::io::Result<()> {
                    let mut file = File::create(&config.filename)?;
                    let writer = AudioWriter {
                        channel: rx_audio,
                        status_channel: tx_status,
                        chunksize,
                        channels: config.channels,
                        samplerate,
                        sample_format: config.format,
                        wav_header: config.wav_header(),
                    };
                    writer.write(&mut file, None);
                    file.flush()
                })() {
                    error!("FileWriter processor '{}' writer error: {}", name_str, err);
                }
            })
            .unwrap();
        ConsumerHandle::new(tx_audio, rx_status, Some(handle))
    }

    fn reconfigure(&mut self, config: config::FileWriterParameters) {
        if config == self.config {
            return;
        }

        self.handles = self.shutdown();

        let handle = Self::start_consumer(&self.name, &config, self.samplerate, self.chunksize);
        self.consumer = handle;
        self.playback_done = false;
        self.config = config;
        debug!(
            "Updated FileWriter processor '{}', channels: {}, filename: {}, format: {}, wav_header: {}, buffer_seconds: {}",
            self.name,
            self.config.channels,
            self.config.filename,
            self.config.format,
            self.config.wav_header(),
            self.config.buffer_seconds()
        );
    }

    fn shutdown(&mut self) -> Vec<JoinHandle<()>> {
        // Stop writer thread
        if !self.playback_done {
            self.consumer
                .channel
                .send(AudioMessage::EndOfStream)
                .unwrap_or(());
        }
        // Pump messages
        self.process_status();
        // Take handles
        let mut handles = std::mem::take(&mut self.handles);
        // Add consumer join handle to pending handles
        if let Some(handle) = self.consumer.handle.take() {
            handles.push(handle);
        }
        // Join finished threads
        let mut index = 0;
        while index < handles.len() {
            if handles[index].is_finished() {
                let handle = handles.swap_remove(index);
                if handle.join().is_err() {
                    warn!("FileWriter thread '{}' panicked", self.name);
                }
            } else {
                index += 1;
            }
        }
        // Return join handles
        handles
    }

    fn process_status(&mut self) -> bool {
        self.consumer
            .status_channel
            .try_iter()
            .for_each(|status| match status {
                StatusMessage::PlaybackError(err) => {
                    error!("FileWriter processor '{}' writer error: {}", self.name, err);
                }
                StatusMessage::PlaybackDone => {
                    self.playback_done = true;
                }
                _ => {}
            });
        self.playback_done
    }
}

impl Drop for FileWriter {
    fn drop(&mut self) {
        let handles = self.shutdown();
        if !handles.is_empty() {
            error!(
                "FileWriter processor '{}' dropped without calling shutdown(). Processing stalled until writer threads join.",
                self.name
            );
        }
        for handle in handles {
            if handle.join().is_err() {
                warn!("FileWriter thread '{}' panicked", self.name);
            }
        }
    }
}

impl Processor for FileWriter {
    fn name(&self) -> &str {
        &self.name
    }

    fn shutdown(&mut self) -> Vec<JoinHandle<()>> {
        self.shutdown()
    }

    /// Copy the input AudioChunk and send it to the consumer thread.
    fn process_chunk(&mut self, input: &mut AudioChunk) -> Res<()> {
        self.process_status();
        if self.playback_done {
            return Ok(());
        }

        let valid_frames = input.valid_frames.min(self.chunksize);
        if valid_frames == 0 {
            self.consumer
                .channel
                .send(AudioMessage::EndOfStream)
                .unwrap_or(());
            return Ok(());
        }

        let chunk = AudioChunk::from(input, input.waveforms.clone());
        match self.consumer.channel.send(AudioMessage::Audio(chunk)) {
            Ok(()) => self.playback_done = false,
            Err(_) => {
                if !self.playback_done {
                    warn!(
                        "FileWriter processor '{}' writer has stopped, dropping samples",
                        self.name
                    );
                    self.playback_done = true;
                }
            }
        };
        Ok(())
    }

    fn update_parameters(&mut self, config: config::Processor) {
        if let config::Processor::FileWriter {
            parameters: config, ..
        } = config
        {
            self.reconfigure(config);
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

/// Validate the file writer config.
pub fn validate_filewriter(config: &config::FileWriterParameters) -> Res<()> {
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
    if !config.buffer_seconds().is_finite() || config.buffer_seconds() <= 0.0 {
        return Err(config::ConfigError::new(
            "FileWriter processor buffer_seconds must be greater than zero.",
        )
        .into());
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
    use crossbeam_channel::TryRecvError;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_test_filename() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut temp_dir = std::env::temp_dir();
        if !temp_dir.exists() {
            temp_dir = std::path::PathBuf::from("/tmp");
        }
        temp_dir
            .join(format!(
                "camilladsp_filewriter_test_{}_{}.raw",
                std::process::id(),
                nanos
            ))
            .to_string_lossy()
            .into_owned()
    }

    /// Shutdown a FileWriter properly, joining all consumer threads.
    fn join_all(mut fw: FileWriter) {
        for handle in fw.shutdown() {
            handle.join().unwrap();
        }
    }

    /// Convenience: stereo f32 raw config with given filename.
    fn f32_config(filename: String) -> config::FileWriterParameters {
        config::FileWriterParameters {
            channels: 2,
            filename,
            format: BinarySampleFormat::F32_LE,
            wav_header: Some(false),
            buffer_seconds: Some(1.0),
        }
    }

    /// Convenience: mono s16 config with given filename.
    fn s16_config(filename: String) -> config::FileWriterParameters {
        config::FileWriterParameters {
            channels: 1,
            filename,
            format: BinarySampleFormat::S16_LE,
            wav_header: Some(false),
            buffer_seconds: Some(2.0),
        }
    }

    /// Make a stereo AudioChunk from 4 sample values.
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

    /// Make a mono AudioChunk from 2 sample values.
    fn mono_chunk(samples: [f64; 2], valid_frames: usize) -> AudioChunk {
        AudioChunk::new(
            vec![vec![samples[0] as PrcFmt, samples[1] as PrcFmt]],
            samples[0].max(samples[1]) as PrcFmt,
            samples[0].min(samples[1]) as PrcFmt,
            2,
            valid_frames,
        )
    }

    /// Build a FileWriter with injected audio sender and status receiver.
    fn filewriter_with_channels(
        channel: Sender<AudioMessage>,
        status_channel: Receiver<StatusMessage>,
        channels: usize,
        chunksize: usize,
    ) -> FileWriter {
        FileWriter {
            name: "filewritertest".to_string(),
            config: config::FileWriterParameters {
                channels,
                filename: unique_test_filename(),
                format: BinarySampleFormat::F32_LE,
                wav_header: Some(false),
                buffer_seconds: Some(1.0),
            },
            samplerate: 48_000,
            chunksize,
            consumer: ConsumerHandle::new(channel, status_channel, None),
            handles: Vec::new(),
            playback_done: false,
        }
    }

    /// Build a FileWriter with injected audio sender (no status channel).
    fn filewriter_with_channel(
        channel: Sender<AudioMessage>,
        channels: usize,
        chunksize: usize,
    ) -> FileWriter {
        filewriter_with_channels(channel, unbounded::<StatusMessage>().1, channels, chunksize)
    }

    #[test]
    fn writes_three_interleaved_raw_chunks_without_modifying_chunks() {
        let filename = unique_test_filename();
        let mut fw = FileWriter::from_config("test", f32_config(filename.clone()), 48_000, 2);

        let mut c1 = stereo_chunk([0.25, -0.5], [0.75, -1.0], 2);
        let mut c2 = stereo_chunk([0.125, -0.25], [0.5, -0.75], 2);
        let mut c3 = stereo_chunk([1.0, -0.875], [0.0, 0.375], 1);

        fw.process_chunk(&mut c1).unwrap();
        fw.process_chunk(&mut c2).unwrap();
        fw.process_chunk(&mut c3).unwrap();
        join_all(fw);

        // Chunks unchanged.
        assert_eq!(c1.waveforms[0], vec![0.25 as PrcFmt, -0.5 as PrcFmt]);
        assert_eq!(c1.waveforms[1], vec![0.75 as PrcFmt, -1.0 as PrcFmt]);
        assert_eq!(c2.waveforms[0], vec![0.125 as PrcFmt, -0.25 as PrcFmt]);
        assert_eq!(c2.waveforms[1], vec![0.5 as PrcFmt, -0.75 as PrcFmt]);
        assert_eq!(c3.waveforms[0], vec![1.0 as PrcFmt, -0.875 as PrcFmt]);
        assert_eq!(c3.waveforms[1], vec![0.0 as PrcFmt, 0.375 as PrcFmt]);

        let data = fs::read(&filename).unwrap();
        let mut expected = Vec::new();
        for &value in &[
            0.25f32, 0.75f32, -0.5f32, -1.0f32, 0.125f32, 0.5f32, -0.25f32, -0.75f32, 1.0f32,
            0.0f32,
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
        join_all(fw);

        assert!(fs::read(&filename).unwrap().is_empty());
        let _ = fs::remove_file(filename);
    }

    #[test]
    fn reconfigure_same_filename_replaces_recording_and_truncates_file() {
        let filename = unique_test_filename();
        let old_config = f32_config(filename.clone());
        let new_config = s16_config(filename.clone());

        let mut fw = FileWriter::from_config("test", old_config, 48_000, 2);

        // Write old-format chunk first.
        let mut old_chunk = stereo_chunk([0.25, -0.5], [0.75, -1.0], 2);
        fw.process_chunk(&mut old_chunk).unwrap();

        // Reconfigure: same filename, new format → teardown + fresh writer.
        fw.reconfigure(new_config.clone());
        assert_eq!(fw.config, new_config);

        // New writer truncates file, write new-format chunk.
        let mut new_chunk = mono_chunk([0.5, -0.5], 2);
        fw.process_chunk(&mut new_chunk).unwrap();
        join_all(fw);

        // File should contain ONLY the new recording (s16 samples).
        let data = fs::read(&filename).unwrap();
        let mut expected = Vec::new();
        expected.extend_from_slice(&16384i16.to_le_bytes());
        expected.extend_from_slice(&(-16384i16).to_le_bytes());
        assert_eq!(data, expected);
        let _ = fs::remove_file(filename);
    }

    #[test]
    fn reconfigure_different_filename_splits_recordings() {
        let old_filename = unique_test_filename();
        let new_filename = unique_test_filename();
        let old_config = f32_config(old_filename.clone());
        let new_config = s16_config(new_filename.clone());

        let mut fw = FileWriter::from_config("test", old_config, 48_000, 2);
        let mut old_chunk = stereo_chunk([0.25, -0.5], [0.75, -1.0], 2);
        let mut new_chunk = mono_chunk([0.5, -0.5], 2);

        fw.process_chunk(&mut old_chunk).unwrap();
        fw.reconfigure(new_config.clone());
        assert_eq!(fw.config, new_config);
        fw.process_chunk(&mut new_chunk).unwrap();
        join_all(fw);

        // Old file has f32 samples.
        let old_data = fs::read(&old_filename).unwrap();
        let mut old_expected = Vec::new();
        for &value in &[0.25f32, 0.75f32, -0.5f32, -1.0f32] {
            old_expected.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(old_data, old_expected);

        // New file has s16 samples.
        let new_data = fs::read(&new_filename).unwrap();
        let mut new_expected = Vec::new();
        new_expected.extend_from_slice(&16384i16.to_le_bytes());
        new_expected.extend_from_slice(&(-16384i16).to_le_bytes());
        assert_eq!(new_data, new_expected);

        let _ = fs::remove_file(old_filename);
        let _ = fs::remove_file(new_filename);
    }

    #[test]
    fn reconfigure_identical_config_is_noop() {
        let filename = unique_test_filename();
        let config = f32_config(filename.clone());
        let mut fw = FileWriter::from_config("test", config.clone(), 48_000, 2);

        fw.reconfigure(config.clone());

        // Consumer thread still running — no teardown occurred.
        assert!(
            fw.consumer
                .handle
                .as_ref()
                .is_some_and(|h| !h.is_finished()),
            "consumer thread should still be running after no-op reconfigure"
        );
        assert_eq!(fw.config, config);
        join_all(fw);
        let _ = fs::remove_file(filename);
    }

    #[test]
    fn process_chunk_sends_audio_message() {
        let (tx, rx) = bounded::<AudioMessage>(1);
        let mut fw = filewriter_with_channel(tx, 2, 2);
        let mut chunk = stereo_chunk([1.0, 2.0], [3.0, 4.0], 2);

        fw.process_chunk(&mut chunk).unwrap();

        let sent = rx.try_recv().expect("audio message should be sent");
        match sent {
            AudioMessage::Audio(sent) => {
                assert_eq!(sent.waveforms[0], vec![1.0 as PrcFmt, 2.0 as PrcFmt]);
                assert_eq!(sent.waveforms[1], vec![3.0 as PrcFmt, 4.0 as PrcFmt]);
                assert_eq!(sent.valid_frames, 2);
            }
            _ => panic!("expected audio message"),
        }
        // Original chunk unmodified.
        assert_eq!(chunk.waveforms[0], vec![1.0 as PrcFmt, 2.0 as PrcFmt]);
    }

    #[test]
    fn process_chunk_skips_zero_valid_frames() {
        let (tx, rx) = bounded::<AudioMessage>(1);
        let mut fw = filewriter_with_channel(tx, 2, 2);
        let mut chunk = stereo_chunk([1.0, 2.0], [3.0, 4.0], 0);

        fw.process_chunk(&mut chunk).unwrap();

        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn playback_error_status_stops_future_writes() {
        let (tx_audio, rx_audio) = bounded::<AudioMessage>(1);
        let (tx_status, rx_status) = unbounded::<StatusMessage>();
        let mut fw = filewriter_with_channels(tx_audio, rx_status, 2, 2);

        // Inject error status from consumer.
        tx_status
            .send(StatusMessage::PlaybackError("boom".into()))
            .unwrap();
        drop(tx_status);

        let mut chunk = stereo_chunk([1.0, 2.0], [3.0, 4.0], 2);
        fw.process_chunk(&mut chunk).unwrap();

        // No audio sent after error status.
        assert!(matches!(rx_audio.try_recv(), Err(TryRecvError::Empty)));
        assert!(fw.playback_done);
    }

    #[test]
    fn playback_done_status_stops_future_writes() {
        let (tx_audio, rx_audio) = bounded::<AudioMessage>(1);
        let (tx_status, rx_status) = unbounded::<StatusMessage>();
        let mut fw = filewriter_with_channels(tx_audio, rx_status, 2, 2);

        tx_status.send(StatusMessage::PlaybackDone).unwrap();
        drop(tx_status);

        let mut chunk = stereo_chunk([1.0, 2.0], [3.0, 4.0], 2);
        fw.process_chunk(&mut chunk).unwrap();

        assert!(matches!(rx_audio.try_recv(), Err(TryRecvError::Empty)));
        assert!(fw.playback_done);
    }

    #[test]
    fn writes_wav_header_and_samples() {
        let filename = unique_test_filename();
        let config = config::FileWriterParameters {
            channels: 1,
            filename: filename.clone(),
            format: BinarySampleFormat::S16_LE,
            wav_header: Some(true),
            buffer_seconds: Some(1.0),
        };
        let mut fw = FileWriter::from_config("test", config, 48_000, 2);
        let mut chunk = mono_chunk([0.5, -0.5], 2);

        fw.process_chunk(&mut chunk).unwrap();
        join_all(fw);

        let data = fs::read(&filename).unwrap();
        // RIFF header present, file bigger than raw samples alone.
        assert!(data.starts_with(b"RIFF"));
        assert!(data.len() > 4 + 24 + 8); // RIFF hdr + fmt chunk + data hdr
        let _ = fs::remove_file(filename);
    }
}
