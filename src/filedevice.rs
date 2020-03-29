extern crate num_traits;
//use std::{iter, error};

use std::fs::File;
use std::io::ErrorKind;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::thread;
//mod audiodevice;
use audiodevice::*;
// Sample format
use config::SampleFormat;
use conversions::{
    buffer_to_chunk_bytes, buffer_to_chunk_float_bytes, chunk_to_buffer_bytes,
    chunk_to_buffer_float_bytes,
};

use CommandMessage;
use PrcFmt;
use Res;
use StatusMessage;

pub struct FilePlaybackDevice {
    pub filename: String,
    pub bufferlength: usize,
    pub samplerate: usize,
    pub channels: usize,
    pub format: SampleFormat,
}

pub struct FileCaptureDevice {
    pub filename: String,
    pub bufferlength: usize,
    pub samplerate: usize,
    pub channels: usize,
    pub format: SampleFormat,
    pub silence_threshold: PrcFmt,
    pub silence_timeout: PrcFmt,
    pub extra_samples: usize,
}

/// Start a playback thread listening for AudioMessages via a channel.
impl PlaybackDevice for FilePlaybackDevice {
    fn start(
        &mut self,
        channel: mpsc::Receiver<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: mpsc::Sender<StatusMessage>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let filename = self.filename.clone();
        let bufferlength = self.bufferlength;
        let channels = self.channels;
        let bits = match self.format {
            SampleFormat::S16LE => 16,
            SampleFormat::S24LE => 24,
            SampleFormat::S32LE => 32,
            SampleFormat::FLOAT32LE => 32,
            SampleFormat::FLOAT64LE => 64,
        };
        let store_bytes = match self.format {
            SampleFormat::S16LE => 2,
            SampleFormat::S24LE => 4,
            SampleFormat::S32LE => 4,
            SampleFormat::FLOAT32LE => 4,
            SampleFormat::FLOAT64LE => 8,
        };
        let format = self.format.clone();
        let handle = thread::spawn(move || {
            //let delay = time::Duration::from_millis((4*1000*bufferlength/samplerate) as u64);
            match File::create(filename) {
                Ok(mut file) => {
                    match status_channel.send(StatusMessage::PlaybackReady) {
                        Ok(()) => {}
                        Err(_err) => {}
                    }
                    //let scalefactor = (1<<bits-1) as PrcFmt;
                    let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
                    barrier.wait();
                    //thread::sleep(delay);
                    debug!("starting playback loop");
                    let mut buffer = vec![0u8; bufferlength * channels * store_bytes];
                    loop {
                        match channel.recv() {
                            Ok(AudioMessage::Audio(chunk)) => {
                                match format {
                                    SampleFormat::S16LE
                                    | SampleFormat::S24LE
                                    | SampleFormat::S32LE => {
                                        chunk_to_buffer_bytes(
                                            chunk,
                                            &mut buffer,
                                            scalefactor,
                                            bits,
                                        );
                                    }
                                    SampleFormat::FLOAT32LE | SampleFormat::FLOAT64LE => {
                                        chunk_to_buffer_float_bytes(chunk, &mut buffer, bits);
                                    }
                                };
                                let write_res = file.write(&buffer);
                                match write_res {
                                    Ok(_) => {}
                                    Err(msg) => {
                                        status_channel
                                            .send(StatusMessage::PlaybackError {
                                                message: format!("{}", msg),
                                            })
                                            .unwrap();
                                    }
                                };
                            }
                            Ok(AudioMessage::EndOfStream) => {
                                status_channel.send(StatusMessage::PlaybackDone).unwrap();
                                break;
                            }
                            Err(_) => {}
                        }
                    }
                }
                Err(err) => {
                    status_channel
                        .send(StatusMessage::PlaybackError {
                            message: format!("{}", err),
                        })
                        .unwrap();
                }
            }
        });
        Ok(Box::new(handle))
    }
}

/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for FileCaptureDevice {
    fn start(
        &mut self,
        channel: mpsc::SyncSender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: mpsc::Sender<StatusMessage>,
        command_channel: mpsc::Receiver<CommandMessage>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let filename = self.filename.clone();
        let samplerate = self.samplerate;
        let bufferlength = self.bufferlength;
        let channels = self.channels;
        let bits = match self.format {
            SampleFormat::S16LE => 16,
            SampleFormat::S24LE => 24,
            SampleFormat::S32LE => 32,
            SampleFormat::FLOAT32LE => 32,
            SampleFormat::FLOAT64LE => 64,
        };
        let store_bytes = match self.format {
            SampleFormat::S16LE => 2,
            SampleFormat::S24LE => 4,
            SampleFormat::S32LE => 4,
            SampleFormat::FLOAT32LE => 4,
            SampleFormat::FLOAT64LE => 8,
        };
        let format = self.format.clone();
        let extra_samples = self.extra_samples;
        let mut silence: PrcFmt = 10.0;
        silence = silence.powf(self.silence_threshold / 20.0);
        let silent_limit =
            (self.silence_timeout * ((samplerate / bufferlength) as PrcFmt)) as usize;
        let handle = thread::spawn(move || {
            match File::open(filename) {
                Ok(mut file) => {
                    match status_channel.send(StatusMessage::CaptureReady) {
                        Ok(()) => {}
                        Err(_err) => {}
                    }
                    let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
                    let mut silent_nbr: usize = 0;
                    barrier.wait();
                    debug!("starting captureloop");
                    let bufferlength_bytes = channels * bufferlength * store_bytes;
                    let mut buf = vec![0u8; bufferlength_bytes];
                    loop {
                        if let Ok(CommandMessage::Exit) = command_channel.try_recv() {
                            let msg = AudioMessage::EndOfStream;
                            channel.send(msg).unwrap();
                            status_channel.send(StatusMessage::CaptureDone).unwrap();
                            break;
                        }
                        //let frames = self.io.readi(&mut buf)?;
                        //let read_res = file.read_exact(&mut buf);
                        //match read_res {
                        //    Ok(_) => {}
                        //    Err(err) => {
                        //        match err.kind() {
                        //            ErrorKind::UnexpectedEof => {
                        //                debug!("Reached end of file");
                        //                send_silence(
                        //                    extra_samples,
                        //                    channels,
                        //                    bufferlength,
                        //                    &channel,
                        //                );
                        //                let msg = AudioMessage::EndOfStream;
                        //                channel.send(msg).unwrap();
                        //                status_channel.send(StatusMessage::CaptureDone).unwrap();
                        //                break;
                        //            }
                        //            _ => status_channel
                        //                .send(StatusMessage::CaptureError {
                        //                    message: format!("{}", err),
                        //                })
                        //                .unwrap(),
                        //        };
                        //    }
                        //};
                        let read_res = read_retry(&mut file, &mut buf);
                        match read_res {
                            Ok(bytes) => {
                                if bytes > 0 && bytes < bufferlength_bytes {
                                    for item in buf.iter_mut().take(bufferlength_bytes).skip(bytes)
                                    {
                                        *item = 0;
                                    }
                                    debug!("Read only {} of {} bytes", bytes, bufferlength_bytes);
                                } else if bytes == 0 {
                                    debug!("Reached end of file");
                                    send_silence(extra_samples, channels, bufferlength, &channel);
                                    let msg = AudioMessage::EndOfStream;
                                    channel.send(msg).unwrap();
                                    status_channel.send(StatusMessage::CaptureDone).unwrap();
                                    break;
                                }
                            }
                            Err(err) => {
                                debug!("Encountered a read error");
                                status_channel
                                    .send(StatusMessage::CaptureError {
                                        message: format!("{}", err),
                                    })
                                    .unwrap();
                            }
                        };

                        //let before = Instant::now();
                        let chunk = match format {
                            SampleFormat::S16LE | SampleFormat::S24LE | SampleFormat::S32LE => {
                                buffer_to_chunk_bytes(&buf, channels, scalefactor, bits)
                            }
                            SampleFormat::FLOAT32LE | SampleFormat::FLOAT64LE => {
                                buffer_to_chunk_float_bytes(&buf, channels, bits)
                            }
                        };
                        if (chunk.maxval - chunk.minval) > silence {
                            if silent_nbr > silent_limit {
                                debug!("Resuming processing");
                            }
                            silent_nbr = 0;
                        } else if silent_limit > 0 {
                            if silent_nbr == silent_limit {
                                debug!("Pausing processing");
                            }
                            silent_nbr += 1;
                        }
                        if silent_nbr <= silent_limit {
                            let msg = AudioMessage::Audio(chunk);
                            channel.send(msg).unwrap();
                        }
                    }
                }
                Err(err) => {
                    status_channel
                        .send(StatusMessage::CaptureError {
                            message: format!("{}", err),
                        })
                        .unwrap();
                }
            }
        });
        Ok(Box::new(handle))
    }
}

fn send_silence(
    samples: usize,
    channels: usize,
    bufferlength: usize,
    audio_channel: &mpsc::SyncSender<AudioMessage>,
) {
    let nchunks = (samples as f32 / bufferlength as f32).ceil() as usize;
    debug!("Sending {} extra chunks", nchunks);
    for _ in 0..nchunks {
        let waveforms = vec![vec![0.0; bufferlength]; channels];
        let chunk = AudioChunk {
            frames: bufferlength,
            channels,
            maxval: 0.0,
            minval: 0.0,
            waveforms,
        };
        let msg = AudioMessage::Audio(chunk);
        audio_channel.send(msg).unwrap();
    }
}

fn read_retry(file: &mut File, mut buf: &mut [u8]) -> Res<usize> {
    let requested = buf.len();
    while !buf.is_empty() {
        match file.read(buf) {
            Ok(0) => break,
            Ok(n) => {
                let tmp = buf;
                buf = &mut tmp[n..];
            }
            Err(ref e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(Box::new(e)),
        }
    }
    if !buf.is_empty() {
        Ok(requested - buf.len())
    } else {
        Ok(requested)
    }
}
