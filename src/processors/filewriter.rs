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
use crate::audiochunk::AudioChunk;
use crate::config;
use crate::config::BinarySampleFormat;
use crate::processors::Processor;
use crate::utils::conversions::chunk_to_buffer_rawbytes_unlogged;
use crate::utils::stash::{
    CHUNK_RESERVE, MAX_CONTAINER_STASH_SIZE, MAX_STASH_SIZE, container_from_stash, recycle_chunk,
    recycle_container, stash_can_spare, vec_from_stash,
};
use crate::utils::wavtools::write_wav_header;
use crossbeam_channel::{Receiver, Sender, bounded};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::thread;

// Minimum number of chunks the writer channel can hold
const MIN_CHUNKS: usize = 4;

/// Token in a FileWriter filename, replaced by the local time when the file is created.
pub const TIMESTAMP_TOKEN: &str = "$timestamp$";

pub struct FileWriter {
    name: String,
    config: config::FileWriterParameters,
    process_channels: Vec<usize>,
    tx: Sender<AudioChunk>,
    warned: bool,
}

/// Everything a writer thread was built from. A rebuilt pipeline reuses a
/// running writer only when this is unchanged, so renaming the processor
/// starts a new file, like any other change.
#[derive(Clone, Debug, PartialEq)]
struct WriterKey {
    name: String,
    params: config::FileWriterParameters,
    samplerate: usize,
    chunksize: usize,
}

struct PoolEntry {
    key: WriterKey,
    tx: Sender<AudioChunk>,
    handle: Option<thread::JoinHandle<()>>,
    used: bool,
}

/// The writer threads of a processing session, keyed on output file.
///
/// The pool outlives the pipelines built from it, so a FileWriter that is
/// unchanged by a config reload keeps its thread and its open file. A writer
/// that has to be replaced hands its thread to the replacement, which joins it
/// before opening the file. At most one thread writes to a file at a time.
#[derive(Default)]
pub struct WriterPool {
    entries: HashMap<PathBuf, PoolEntry>,
    retired: Vec<thread::JoinHandle<()>>,
}

impl WriterPool {
    /// Start a pipeline build. Writers the build does not ask for are retired
    /// by the following [`WriterPool::sweep`].
    pub fn start_build(&mut self) {
        for entry in self.entries.values_mut() {
            entry.used = false;
        }
    }

    /// Get a sender to the writer for these parameters, starting one if needed.
    fn sender(&mut self, name: &str, key: WriterKey, nbr_channels: usize) -> Sender<AudioChunk> {
        let path =
            file_key(&key.params.filename).unwrap_or_else(|_| PathBuf::from(&key.params.filename));
        let predecessor = match self.entries.get_mut(&path) {
            Some(entry) if entry.key == key => {
                debug!(
                    "FileWriter processor '{}' reuses the writer for {}",
                    name, key.params.filename
                );
                entry.used = true;
                return entry.tx.clone();
            }
            Some(_) => self.entries.remove(&path).and_then(|entry| entry.handle),
            None => None,
        };
        let (tx, handle) = spawn_writer(name, key.clone(), nbr_channels, predecessor);
        self.entries.insert(
            path,
            PoolEntry {
                key,
                tx: tx.clone(),
                handle,
                used: true,
            },
        );
        tx
    }

    /// Let go of the writers the latest pipeline does not use.
    ///
    /// Call after the previous pipeline is dropped. Their threads finish writing
    /// what is queued and are joined by [`WriterPool::join`].
    pub fn sweep(&mut self) {
        let retired = &mut self.retired;
        self.entries.retain(|_, entry| {
            if entry.used {
                true
            } else {
                retired.extend(entry.handle.take());
                false
            }
        });
        // Dropping the handle of a finished thread is just bookkeeping.
        retired.retain(|handle| !handle.is_finished());
    }

    /// Close all writers and wait for them to write what is queued.
    ///
    /// Blocks, so call it off the real-time thread and after every pipeline
    /// built from this pool is dropped.
    pub fn join(self) {
        let handles = self
            .entries
            .into_values()
            .filter_map(|entry| entry.handle)
            .chain(self.retired);
        for handle in handles {
            let _ = handle.join();
        }
    }
}

/// The number of chunks a writer can queue: about one second, as far as half
/// the stash can supply it.
fn writer_capacity(samplerate: usize, chunksize: usize, nbr_channels: usize) -> usize {
    let one_second = samplerate / chunksize.max(1);
    let containers = MAX_CONTAINER_STASH_SIZE / 2;
    let vecs = MAX_STASH_SIZE / (2 * nbr_channels.max(1));
    one_second.min(containers).min(vecs).max(MIN_CHUNKS)
}

/// Start a writer thread, and put the chunks it can queue into the stash, plus
/// the reserve it must leave there, so that the stash has them to spare when
/// the writer falls behind.
fn spawn_writer(
    name: &str,
    key: WriterKey,
    nbr_channels: usize,
    predecessor: Option<thread::JoinHandle<()>>,
) -> (Sender<AudioChunk>, Option<thread::JoinHandle<()>>) {
    let capacity = writer_capacity(key.samplerate, key.chunksize, nbr_channels);
    for _ in 0..capacity + CHUNK_RESERVE {
        recycle_container(vec![vec![0.0; key.chunksize]; nbr_channels]);
    }
    let (tx, rx) = bounded::<AudioChunk>(capacity);
    let proc_name = name.to_string();
    let handle = thread::Builder::new()
        .name(format!("FileWriter-{proc_name}"))
        .spawn(move || {
            if let Some(predecessor) = predecessor {
                let _ = predecessor.join();
            }
            if let Err(err) = write_loop(&proc_name, &key, rx) {
                error!("FileWriter processor '{}' writer error: {}", proc_name, err);
            }
        });
    match handle {
        Ok(handle) => (tx, Some(handle)),
        Err(err) => {
            error!(
                "FileWriter processor '{}' failed to spawn writer thread: {}",
                name, err
            );
            (tx, None)
        }
    }
}

/// Write chunks until every sender is dropped. The file is created on the first chunk.
fn write_loop(name: &str, key: &WriterKey, rx: Receiver<AudioChunk>) -> Res<()> {
    let params = &key.params;
    let mut file: Option<BufWriter<File>> = None;
    let mut bytes = Vec::new();
    for chunk in rx.iter() {
        if file.is_none() {
            let filename = resolve_timestamp(&params.filename, chrono::Local::now());
            info!("FileWriter processor '{name}' writing to {filename}");
            let mut f = BufWriter::new(File::create(&filename)?);
            if params.wav_header() {
                write_wav_header(&mut f, chunk.channels, params.format, key.samplerate)?;
            }
            file = Some(f);
        }
        bytes.resize(
            chunk.frames * chunk.channels * params.format.bytes_per_sample(),
            0,
        );
        let (valid_bytes, clipped, peak) =
            chunk_to_buffer_rawbytes_unlogged(chunk, &mut bytes, &params.format);
        if clipped > 0 {
            warn!(
                "FileWriter processor '{}' clipped {} samples, peak +{:.2} dB ({:.1}%)",
                name,
                clipped,
                20.0 * peak.log10(),
                peak * 100.0
            );
        }
        file.as_mut().unwrap().write_all(&bytes[..valid_bytes])?;
    }
    if let Some(mut file) = file {
        file.flush()?;
    }
    debug!("FileWriter writer for '{}' done", params.filename);
    Ok(())
}

/// Replace the timestamp token with the local time, to the second.
fn resolve_timestamp<Tz: chrono::TimeZone>(filename: &str, now: chrono::DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    if !filename.contains(TIMESTAMP_TOKEN) {
        return filename.to_string();
    }
    let stamp = now.format("%Y%m%d-%H%M%S").to_string();
    filename.replace(TIMESTAMP_TOKEN, &stamp)
}

impl FileWriter {
    /// Creates a FileWriter processor from a config struct.
    pub fn from_config(
        name: &str,
        config: config::FileWriterParameters,
        samplerate: usize,
        chunksize: usize,
        pool: &mut WriterPool,
    ) -> Self {
        debug!(
            "Creating FileWriter processor '{}', channels: {}, process_channels: {:?}, filename: {}, format: {}",
            name,
            config.channels,
            config.process_channels(),
            config.filename,
            config.format
        );
        let mut process_channels = config.process_channels();
        if process_channels.is_empty() {
            process_channels = (0..config.channels).collect();
        }
        let key = WriterKey {
            name: name.to_string(),
            params: config.clone(),
            samplerate,
            chunksize,
        };
        let tx = pool.sender(name, key, process_channels.len());
        FileWriter {
            name: name.to_string(),
            config,
            process_channels,
            tx,
            warned: false,
        }
    }
}

impl Processor for FileWriter {
    fn name(&self) -> &str {
        &self.name
    }

    /// Copy the selected channels of the input AudioChunk and queue them for writing.
    fn process_chunk(&mut self, input: &mut AudioChunk) {
        // Drop the chunk rather than take buffers the audio path may need.
        if self.tx.is_full() || !stash_can_spare(self.process_channels.len()) {
            if !self.warned {
                warn!(
                    "FileWriter processor '{}' buffer overrun, dropping chunks",
                    self.name
                );
                self.warned = true;
            }
            return;
        }
        let mut waveforms = container_from_stash(self.process_channels.len());
        for channel in self.process_channels.iter() {
            let source = &input.waveforms[*channel];
            // An empty waveform is a silent channel, and stays empty.
            let mut waveform = if source.is_empty() {
                Vec::new()
            } else {
                vec_from_stash(source.len())
            };
            waveform.copy_from_slice(source);
            waveforms.push(waveform);
        }
        let chunk = AudioChunk::from(input, waveforms);
        match self.tx.try_send(chunk) {
            Ok(()) => {
                let capacity = self.tx.capacity().unwrap_or_default();
                if self.warned && self.tx.len() * 2 <= capacity {
                    self.warned = false;
                }
            }
            // Only this thread sends, so after the check above an error means
            // the writer thread stopped, and it logged why.
            Err(err) => recycle_chunk(err.into_inner()),
        }
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

/// The key that identifies an output file: its canonical parent directory
/// joined with the file name.
///
/// The file itself need not exist, but its directory must.
pub fn file_key(filename: &str) -> std::io::Result<PathBuf> {
    let path = Path::new(filename);
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("the path has no file name"))?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(std::fs::canonicalize(parent)?.join(name))
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
    let in_directory = Path::new(&config.filename)
        .parent()
        .is_some_and(|dir| dir.to_string_lossy().contains(TIMESTAMP_TOKEN));
    if in_directory {
        return Err(config::ConfigError::new(
            "FileWriter processor filename can only use $timestamp$ in the file name, not the directory.",
        )
        .into());
    }
    if config.wav_header() && config.format == BinarySampleFormat::S24_4_RJ_LE {
        return Err(config::ConfigError::new(
            "Wav files do not support the S24_4_RJ_LE sample format",
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CamillaFloat;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

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

    fn f32_config(filename: String) -> config::FileWriterParameters {
        config::FileWriterParameters {
            channels: 2,
            process_channels: None,
            filename,
            format: BinarySampleFormat::F32_LE,
            wav_header: Some(false),
        }
    }

    fn writer(config: config::FileWriterParameters, pool: &mut WriterPool) -> FileWriter {
        FileWriter::from_config("test", config, 48_000, 2, pool)
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

    fn f32_bytes(values: &[f32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    #[test]
    fn validate_rejects_invalid_process_channel() {
        let mut config = f32_config("test".to_string());
        config.process_channels = Some(vec![2]);

        assert!(validate_file_writer(&config).is_err());
    }

    #[test]
    fn writes_interleaved_raw_chunks_without_modifying_chunks() {
        let filename = unique_test_filename();
        let mut pool = WriterPool::default();
        let mut fw = writer(f32_config(filename.clone()), &mut pool);

        let mut c1 = stereo_chunk([0.25, -0.5], [0.75, -1.0], 2);
        let mut c2 = stereo_chunk([0.125, -0.25], [0.5, -0.75], 2);
        // A partial chunk writes its valid frames only.
        let mut c3 = stereo_chunk([1.0, -0.875], [0.0, 0.375], 1);

        fw.process_chunk(&mut c1);
        fw.process_chunk(&mut c2);
        fw.process_chunk(&mut c3);
        drop(fw);
        pool.join();

        assert_eq!(c1.waveforms[0], vec![0.25, -0.5]);
        assert_eq!(c1.waveforms[1], vec![0.75, -1.0]);
        assert_eq!(c2.waveforms[0], vec![0.125, -0.25]);
        assert_eq!(c2.waveforms[1], vec![0.5, -0.75]);
        assert_eq!(c3.waveforms[0], vec![1.0, -0.875]);
        assert_eq!(c3.waveforms[1], vec![0.0, 0.375]);

        let data = fs::read(&filename).unwrap();
        let expected = f32_bytes(&[0.25, 0.75, -0.5, -1.0, 0.125, 0.5, -0.25, -0.75, 1.0, 0.0]);
        assert_eq!(data, expected);
        let _ = fs::remove_file(&filename);
    }

    #[test]
    fn writes_only_process_channels_in_configured_order() {
        let filename = unique_test_filename();
        let mut config = f32_config(filename.clone());
        config.process_channels = Some(vec![1, 0]);
        let mut pool = WriterPool::default();
        let mut fw = writer(config, &mut pool);

        let mut chunk = stereo_chunk([0.25, -0.5], [0.75, -1.0], 2);

        fw.process_chunk(&mut chunk);
        drop(fw);
        pool.join();

        let data = fs::read(&filename).unwrap();
        assert_eq!(data, f32_bytes(&[0.75, 0.25, -1.0, -0.5]));
        let _ = fs::remove_file(&filename);
    }

    #[test]
    fn drops_chunk_when_channel_is_full() {
        let filename = unique_test_filename();
        let mut pool = WriterPool::default();
        let mut fw = writer(f32_config(filename.clone()), &mut pool);
        // Swap in a channel nobody reads.
        let (tx, rx) = bounded(2);
        fw.tx = tx;

        let mut chunk = stereo_chunk([1.0, 2.0], [3.0, 4.0], 2);
        fw.process_chunk(&mut chunk);
        fw.process_chunk(&mut chunk);
        assert!(!fw.warned);
        fw.process_chunk(&mut chunk);
        assert!(fw.warned);
        assert_eq!(rx.len(), 2);

        // The writer catches up.
        recycle_chunk(rx.recv().unwrap());
        recycle_chunk(rx.recv().unwrap());
        fw.process_chunk(&mut chunk);
        assert!(!fw.warned);

        drop(fw);
        pool.join();
        assert!(!Path::new(&filename).exists());
    }

    #[test]
    fn writer_capacity_fits_in_the_stash() {
        // One second when the stash has room.
        assert_eq!(writer_capacity(48_000, 1024, 2), 46);
        // Half the containers at a small chunksize.
        assert_eq!(writer_capacity(48_000, 64, 2), MAX_CONTAINER_STASH_SIZE / 2);
        // Half the waveform buffers with many channels.
        assert_eq!(writer_capacity(48_000, 64, 32), MAX_STASH_SIZE / 64);
        // Never below the minimum.
        assert_eq!(writer_capacity(1_000, 1024, 2), MIN_CHUNKS);
    }

    #[test]
    fn timestamp_is_resolved_in_the_file_name() {
        use chrono::TimeZone;
        let now = chrono::Utc.with_ymd_and_hms(2026, 9, 23, 8, 5, 3).unwrap();
        assert_eq!(
            resolve_timestamp("/tmp/cap_$timestamp$.wav", now),
            "/tmp/cap_20260923-080503.wav"
        );
        assert_eq!(resolve_timestamp("/tmp/cap.wav", now), "/tmp/cap.wav");

        let mut config = f32_config("/tmp/$timestamp$/cap.wav".to_string());
        assert!(validate_file_writer(&config).is_err());
        config.filename = "/tmp/cap_$timestamp$.wav".to_string();
        assert!(validate_file_writer(&config).is_ok());
    }

    #[test]
    fn update_parameters_panics_on_changed_config() {
        let filename = unique_test_filename();
        let config = f32_config(filename.clone());
        let mut config2 = f32_config(filename.clone());
        config2.channels = config.channels + 1;

        let mut pool = WriterPool::default();
        let mut fw = writer(config.clone(), &mut pool);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fw.update_parameters(config::Processor::FileWriter {
                description: None,
                parameters: config2,
            });
        }));
        assert!(result.is_err());

        drop(fw);
        pool.join();
    }

    #[test]
    fn unchanged_writer_survives_a_rebuild() {
        let filename = unique_test_filename();
        let mut pool = WriterPool::default();
        let mut old = writer(f32_config(filename.clone()), &mut pool);
        old.process_chunk(&mut stereo_chunk([0.25, -0.5], [0.75, -1.0], 2));

        // Build the new pipeline before dropping the old, as processing does.
        let mut new = writer(f32_config(filename.clone()), &mut pool);
        drop(old);
        pool.sweep();
        new.process_chunk(&mut stereo_chunk([0.125, -0.25], [0.5, -0.75], 2));
        drop(new);
        pool.join();

        let data = fs::read(&filename).unwrap();
        let expected = f32_bytes(&[0.25, 0.75, -0.5, -1.0, 0.125, 0.5, -0.25, -0.75]);
        assert_eq!(data, expected);
        let _ = fs::remove_file(&filename);
    }

    #[test]
    fn changed_writer_waits_for_its_predecessor() {
        let filename = unique_test_filename();
        let mut pool = WriterPool::default();
        let mut old = writer(f32_config(filename.clone()), &mut pool);
        old.process_chunk(&mut stereo_chunk([0.25, -0.5], [0.75, -1.0], 2));

        let mut config = f32_config(filename.clone());
        config.process_channels = Some(vec![1]);
        let mut new = writer(config, &mut pool);
        // The new writer gets data while the old one still holds the file.
        new.process_chunk(&mut stereo_chunk([0.125, -0.25], [0.5, -0.75], 2));
        drop(old);
        pool.sweep();
        drop(new);
        pool.join();

        // The old file is truncated, but only once the old writer is done with it.
        let data = fs::read(&filename).unwrap();
        assert_eq!(data, f32_bytes(&[0.5, -0.75]));
        let _ = fs::remove_file(&filename);
    }

    #[test]
    fn renamed_writer_starts_a_new_file() {
        let filename = unique_test_filename();
        let mut pool = WriterPool::default();
        let mut old =
            FileWriter::from_config("before", f32_config(filename.clone()), 48_000, 2, &mut pool);
        old.process_chunk(&mut stereo_chunk([0.25, -0.5], [0.75, -1.0], 2));

        let mut new =
            FileWriter::from_config("after", f32_config(filename.clone()), 48_000, 2, &mut pool);
        drop(old);
        pool.sweep();
        new.process_chunk(&mut stereo_chunk([0.125, -0.25], [0.5, -0.75], 2));
        drop(new);
        pool.join();

        let data = fs::read(&filename).unwrap();
        assert_eq!(data, f32_bytes(&[0.125, 0.5, -0.25, -0.75]));
        let _ = fs::remove_file(&filename);
    }

    #[test]
    fn dropped_writer_is_retired_and_joined() {
        let filename = unique_test_filename();
        let mut pool = WriterPool::default();
        let mut fw = writer(f32_config(filename.clone()), &mut pool);
        fw.process_chunk(&mut stereo_chunk([0.25, -0.5], [0.75, -1.0], 2));
        // A rebuild without this writer.
        pool.start_build();
        drop(fw);
        pool.sweep();
        assert!(pool.entries.is_empty());
        pool.join();

        let data = fs::read(&filename).unwrap();
        assert_eq!(data, f32_bytes(&[0.25, 0.75, -0.5, -1.0]));
        let _ = fs::remove_file(&filename);
    }

    #[test]
    fn file_key_folds_the_parent_directory() {
        let dir = std::env::temp_dir();
        let plain = dir.join("capture.raw");
        let dotted = dir.join(".").join("capture.raw");
        assert_eq!(
            file_key(plain.to_str().unwrap()).unwrap(),
            file_key(dotted.to_str().unwrap()).unwrap()
        );
        assert!(file_key("/no/such/dir/capture.raw").is_err());
    }

    #[test]
    fn writes_wav_header_when_enabled() {
        let filename = unique_test_filename();
        let mut config = f32_config(filename.clone());
        config.wav_header = Some(true);
        let mut pool = WriterPool::default();
        let mut fw = writer(config, &mut pool);
        fw.process_chunk(&mut stereo_chunk([0.25, -0.5], [0.75, -1.0], 2));
        drop(fw);
        pool.join();
        let data = fs::read(&filename).unwrap();
        assert!(data.starts_with(b"RIFF"));
        assert!(data.len() > 4 + 24 + 8);
        let _ = fs::remove_file(&filename);
    }

    #[test]
    fn validate_rejects_right_justified_wav() {
        let mut config = f32_config("test.wav".to_string());
        config.format = BinarySampleFormat::S24_4_RJ_LE;
        config.wav_header = Some(true);
        assert!(validate_file_writer(&config).is_err());
    }
}
