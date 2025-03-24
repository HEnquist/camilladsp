use crate::audiodevice::*;
use crate::config;
use crate::config::{ConfigError, SampleFormat};
use crate::conversions::{buffer_to_chunk_rawbytes, chunk_to_buffer_rawbytes};
use crate::countertimer;
use crate::helpers::PIRateController;
use crossbeam_channel::{bounded, unbounded, Receiver, Sender, TryRecvError, TrySendError};
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use ringbuf::wrap::caching::Caching;
use ringbuf::{traits::*, HeapRb};
use rubato::VecResampler;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;
use wasapi;
use wasapi::DeviceCollection;

use audio_thread_priority::{
    demote_current_thread_from_real_time, promote_current_thread_to_real_time,
};

use crate::CommandMessage;
use crate::PrcFmt;
use crate::ProcessingParameters;
use crate::ProcessingState;
use crate::Res;
use crate::StatusMessage;
use crate::{CaptureStatus, PlaybackStatus};

enum DeviceState {
    Ok,
    Error(String),
}

#[derive(Clone, Debug)]
pub struct WasapiPlaybackDevice {
    pub devname: Option<String>,
    pub exclusive: bool,
    pub samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
    pub sample_format: SampleFormat,
    pub target_level: usize,
    pub adjust_period: f32,
    pub enable_rate_adjust: bool,
}

#[derive(Clone, Debug)]
pub struct WasapiCaptureDevice {
    pub devname: Option<String>,
    pub exclusive: bool,
    pub loopback: bool,
    pub samplerate: usize,
    pub resampler_config: Option<config::Resampler>,
    pub capture_samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
    pub sample_format: SampleFormat,
    pub silence_threshold: PrcFmt,
    pub silence_timeout: PrcFmt,
    pub stop_on_rate_change: bool,
    pub rate_measure_interval: f32,
}

#[derive(Clone, Debug)]
enum DisconnectReason {
    FormatChange,
    Error,
}

pub fn list_device_names(input: bool) -> Vec<(String, String)> {
    let direction = if input {
        wasapi::Direction::Capture
    } else {
        wasapi::Direction::Render
    };
    let collection = wasapi::DeviceCollection::new(&direction);
    let names = collection
        .map(|coll| list_device_names_in_collection(&coll).unwrap_or_default())
        .unwrap_or_default();
    names.iter().map(|n| (n.clone(), n.clone())).collect()
}

fn list_device_names_in_collection(collection: &DeviceCollection) -> Res<Vec<String>> {
    let mut names = Vec::new();
    let count = collection.get_nbr_devices()?;
    for n in 0..count {
        let device = collection.get_device_at_index(n)?;
        let name = device.get_friendlyname()?;
        names.push(name);
    }
    Ok(names)
}

fn wave_format(
    sample_format: &SampleFormat,
    samplerate: usize,
    channels: usize,
) -> wasapi::WaveFormat {
    match sample_format {
        SampleFormat::S16LE => {
            wasapi::WaveFormat::new(16, 16, &wasapi::SampleType::Int, samplerate, channels, None)
        }
        SampleFormat::S24LE => {
            wasapi::WaveFormat::new(32, 24, &wasapi::SampleType::Int, samplerate, channels, None)
        }
        SampleFormat::S24LE3 => {
            wasapi::WaveFormat::new(24, 24, &wasapi::SampleType::Int, samplerate, channels, None)
        }
        SampleFormat::S32LE => {
            wasapi::WaveFormat::new(32, 32, &wasapi::SampleType::Int, samplerate, channels, None)
        }
        SampleFormat::FLOAT32LE => wasapi::WaveFormat::new(
            32,
            32,
            &wasapi::SampleType::Float,
            samplerate,
            channels,
            None,
        ),
        _ => panic!("Unsupported sample format"),
    }
}

fn get_supported_wave_format(
    audio_client: &wasapi::AudioClient,
    sample_format: &SampleFormat,
    samplerate: usize,
    channels: usize,
    sharemode: &wasapi::ShareMode,
) -> Res<wasapi::WaveFormat> {
    let wave_format = wave_format(sample_format, samplerate, channels);
    match sharemode {
        wasapi::ShareMode::Exclusive => {
            Ok(audio_client.is_supported_exclusive_with_quirks(&wave_format)?)
        }
        wasapi::ShareMode::Shared => match audio_client.is_supported(&wave_format, sharemode) {
            Ok(None) => {
                debug!("Device supports format {:?}.", wave_format);
                Ok(wave_format)
            }
            Ok(Some(modified)) => {
                let msg = format!(
                    "Device doesn't support format:\n{:#?}\nClosest match is:\n{:#?}",
                    wave_format, modified
                );
                Err(ConfigError::new(&msg).into())
            }
            Err(err) => {
                let msg = format!(
                    "Device doesn't support format:\n{:#?}\nError: {}",
                    wave_format, err
                );
                Err(ConfigError::new(&msg).into())
            }
        },
    }
}

fn open_playback(
    devname: &Option<String>,
    samplerate: usize,
    channels: usize,
    sample_format: &SampleFormat,
    exclusive: bool,
) -> Res<(
    wasapi::Device,
    wasapi::AudioClient,
    wasapi::AudioRenderClient,
    wasapi::Handle,
    wasapi::WaveFormat,
)> {
    let sharemode = match exclusive {
        true => wasapi::ShareMode::Exclusive,
        false => wasapi::ShareMode::Shared,
    };
    let device = if let Some(name) = devname {
        let collection = wasapi::DeviceCollection::new(&wasapi::Direction::Render)?;
        debug!(
            "Available playback devices: {:?}.",
            list_device_names_in_collection(&collection)
        );
        collection.get_device_with_name(name)?
    } else {
        wasapi::get_default_device(&wasapi::Direction::Render)?
    };
    trace!("Found playback device {:?}.", devname);
    let mut audio_client = device.get_iaudioclient()?;
    trace!("Got playback iaudioclient.");
    let wave_format = get_supported_wave_format(
        &audio_client,
        sample_format,
        samplerate,
        channels,
        &sharemode,
    )?;
    let (def_time, min_time) = audio_client.get_periods()?;
    let aligned_time =
        audio_client.calculate_aligned_period_near(def_time, Some(128), &wave_format)?;
    audio_client.initialize_client(
        &wave_format,
        aligned_time,
        &wasapi::Direction::Render,
        &sharemode,
        false,
    )?;
    debug!(
        "Playback default period {}, min period {}, aligned period {}.",
        def_time, min_time, aligned_time
    );
    debug!("Initialized playback audio client.");
    let handle = audio_client.set_get_eventhandle()?;
    let render_client = audio_client.get_audiorenderclient()?;
    debug!("Opened Wasapi playback device {:?}.", devname);
    Ok((device, audio_client, render_client, handle, wave_format))
}

fn open_capture(
    devname: &Option<String>,
    samplerate: usize,
    channels: usize,
    sample_format: &SampleFormat,
    exclusive: bool,
    loopback: bool,
) -> Res<(
    wasapi::Device,
    wasapi::AudioClient,
    wasapi::AudioCaptureClient,
    wasapi::Handle,
    wasapi::WaveFormat,
)> {
    let sharemode = match exclusive {
        true => wasapi::ShareMode::Exclusive,
        false => wasapi::ShareMode::Shared,
    };
    let device = if let Some(name) = devname {
        if !loopback {
            let collection = wasapi::DeviceCollection::new(&wasapi::Direction::Capture)?;
            debug!(
                "Available capture devices: {:?}.",
                list_device_names_in_collection(&collection)
            );
            collection.get_device_with_name(name)?
        } else {
            let collection = wasapi::DeviceCollection::new(&wasapi::Direction::Render)?;
            debug!(
                "Available loopback capture (i.e. playback) devices: {:?}.",
                list_device_names_in_collection(&collection)
            );
            collection.get_device_with_name(name)?
        }
    } else if !loopback {
        wasapi::get_default_device(&wasapi::Direction::Capture)?
    } else {
        wasapi::get_default_device(&wasapi::Direction::Render)?
    };

    trace!("Found capture device {:?}.", devname);
    let mut audio_client = device.get_iaudioclient()?;
    trace!("Got capture iaudioclient.");
    let wave_format = get_supported_wave_format(
        &audio_client,
        sample_format,
        samplerate,
        channels,
        &sharemode,
    )?;
    let (def_time, min_time) = audio_client.get_periods()?;
    debug!(
        "Capture default period {}, min period {}.",
        def_time, min_time
    );
    audio_client.initialize_client(
        &wave_format,
        def_time,
        &wasapi::Direction::Capture,
        &sharemode,
        loopback,
    )?;
    debug!("Initialized capture audio client.");
    let handle = audio_client.set_get_eventhandle()?;
    trace!("Capture got event handle.");
    let capture_client = audio_client.get_audiocaptureclient()?;
    debug!("Opened Wasapi capture device {:?}.", devname);
    Ok((device, audio_client, capture_client, handle, wave_format))
}

struct PlaybackSync {
    rx_play: Receiver<PlaybackDeviceMessage>,
    tx_cb: Sender<DisconnectReason>,
    bufferfill: Arc<Mutex<countertimer::DeviceBufferEstimator>>,
}

enum PlaybackDeviceMessage {
    Data(usize),
    Stop,
}

// Playback loop, play samples received from channel
#[allow(clippy::too_many_arguments)]
fn playback_loop(
    audio_client: wasapi::AudioClient,
    render_client: wasapi::AudioRenderClient,
    handle: wasapi::Handle,
    blockalign: usize,
    chunksize: usize,
    samplerate: f64,
    sync: PlaybackSync,
    mut ringbuffer: Caching<Arc<HeapRb<u8>>, false, true>,
    target_level: usize,
) -> Res<()> {
    let mut buffer_free_frame_count = audio_client.get_bufferframecount()?;
    let mut sample_queue: VecDeque<u8> = VecDeque::with_capacity(
        4 * blockalign * (chunksize + 2 * buffer_free_frame_count as usize)
            + target_level * blockalign,
    );

    let tx_cb = sync.tx_cb;
    let mut callbacks = wasapi::EventCallbacks::new();
    callbacks.set_disconnected_callback(move |reason| {
        debug!("Disconnected, reason: {:?}.", reason);
        let simplereason = match reason {
            wasapi::DisconnectReason::FormatChanged => DisconnectReason::FormatChange,
            _ => DisconnectReason::Error,
        };
        tx_cb.send(simplereason).unwrap_or(());
    });
    let callbacks_rc = Arc::new(callbacks);
    let callbacks_weak = Arc::downgrade(&callbacks_rc);
    let sessioncontrol = audio_client.get_audiosessioncontrol()?;
    let clock = audio_client.get_audioclock()?;
    let _registration = sessioncontrol.register_session_notification(callbacks_weak)?;

    let mut waited_millis = 0;
    trace!("Waiting for data to start playback, will time out after one second.");
    while sync.rx_play.len() < 2 && waited_millis < 1000 {
        thread::sleep(Duration::from_millis(10));
        waited_millis += 10;
    }
    debug!("Waited for data for {} ms.", waited_millis);

    // Raise priority
    let _thread_handle = match promote_current_thread_to_real_time(0, 1) {
        Ok(h) => {
            debug!("Playback inner thread has real-time priority.");
            Some(h)
        }
        Err(err) => {
            warn!(
                "Playback inner thread could not get real time priority, error: {}.",
                err
            );
            None
        }
    };

    audio_client.start_stream()?;
    let mut running = false;
    let mut starting = true;
    let mut pos = 0;
    let mut device_prevtime = 0.0;
    let device_freq = clock.get_frequency()? as f64;
    loop {
        buffer_free_frame_count = audio_client.get_available_space_in_frames()?;
        trace!("New buffer frame count {}.", buffer_free_frame_count);
        let mut device_time = pos as f64 / device_freq;
        if device_time == 0.0 && device_prevtime > 0.0 {
            debug!("Failed to get accurate device time, skipping check for missing events.");
            // A zero value means that the device position read was delayed due to some
            // other high priority event, and an accurate reading could not be taken.
            // To avoid needless resets of the stream, set the position to the expected value,
            // calculated as the previous value plus the expected increment.
            device_time = device_prevtime + buffer_free_frame_count as f64 / samplerate;
        }
        trace!(
            "Device time counted up by {:.4} s.",
            device_time - device_prevtime
        );
        if buffer_free_frame_count > 0
            && (device_time - device_prevtime)
                > 1.75 * (buffer_free_frame_count as f64 / samplerate)
        {
            warn!(
                "Missing event! Resetting stream. Interval {:.4} s, expected {:.4} s.",
                device_time - device_prevtime,
                buffer_free_frame_count as f64 / samplerate
            );
            audio_client.stop_stream()?;
            audio_client.reset_stream()?;
            audio_client.start_stream()?;
        }
        device_prevtime = device_time;

        while sample_queue.len() < (blockalign * buffer_free_frame_count as usize) {
            trace!("Playback loop needs more samples, reading from channel.");
            match sync.rx_play.try_recv() {
                Ok(PlaybackDeviceMessage::Data(bytes)) => {
                    trace!("Received chunk.");
                    if !running {
                        running = true;
                        if starting {
                            starting = false;
                        } else {
                            warn!("Restarting playback after buffer underrun.");
                        }
                        debug!("Inserting {target_level} silent frames to reach target delay.");
                        for _ in 0..(blockalign * target_level) {
                            sample_queue.push_back(0);
                        }
                    }
                    for element in ringbuffer.pop_iter().take(bytes) {
                        sample_queue.push_back(element);
                    }
                }
                Ok(PlaybackDeviceMessage::Stop) => {
                    debug!("Stopping inner playback loop.");
                    audio_client.stop_stream()?;
                    return Ok(());
                }
                Err(TryRecvError::Empty) => {
                    for _ in
                        0..((blockalign * buffer_free_frame_count as usize) - sample_queue.len())
                    {
                        sample_queue.push_back(0);
                    }
                    if running {
                        running = false;
                        warn!("Playback interrupted, no data available.");
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    error!("Channel is closed.");
                    return Err(DeviceError::new("Data channel closed.").into());
                }
            }
        }
        render_client.write_to_device_from_deque(
            buffer_free_frame_count as usize,
            &mut sample_queue,
            None,
        )?;
        let curr_buffer_fill = sample_queue.len() / blockalign + sync.rx_play.len() * chunksize;
        if let Ok(mut estimator) = sync.bufferfill.try_lock() {
            estimator.add(curr_buffer_fill)
        }
        trace!("Write ok.");
        //println!("{} bef",prev_inst.elapsed().as_micros());
        if handle.wait_for_event(1000).is_err() {
            error!("Error on playback, stopping stream");
            audio_client.stop_stream()?;
            return Err(DeviceError::new("Error on playback").into());
        }

        pos = clock.get_position()?.0;
    }
}

struct CaptureChannels {
    pub tx_filled: Sender<(u64, usize)>,
    pub ringbuf: Caching<Arc<HeapRb<u8>>, true, false>,
}

// Capture loop, capture samples and send in chunks of "chunksize" frames to channel
fn capture_loop(
    audio_client: wasapi::AudioClient,
    capture_client: wasapi::AudioCaptureClient,
    handle: wasapi::Handle,
    mut channels: CaptureChannels,
    tx_disconnectreason: Sender<DisconnectReason>,
    blockalign: usize,
    stop_signal: Arc<AtomicBool>,
) -> Res<()> {
    let mut chunk_nbr: u64 = 0;

    let mut callbacks = wasapi::EventCallbacks::new();
    callbacks.set_disconnected_callback(move |reason| {
        debug!("Capture disconnected, reason: {:?}.", reason);
        let simplereason = match reason {
            wasapi::DisconnectReason::FormatChanged => DisconnectReason::FormatChange,
            _ => DisconnectReason::Error,
        };
        tx_disconnectreason.send(simplereason).unwrap_or(());
    });

    let callbacks_rc = Arc::new(callbacks);
    let callbacks_weak = Arc::downgrade(&callbacks_rc);

    let sessioncontrol = audio_client.get_audiosessioncontrol()?;
    let _registration = sessioncontrol.register_session_notification(callbacks_weak)?;

    let mut inactive = false;

    let mut data = vec![0u8; 8 * blockalign * 1024];

    // Raise priority
    let _thread_handle = match promote_current_thread_to_real_time(0, 1) {
        Ok(h) => {
            debug!("Capture inner thread has real-time priority.");
            Some(h)
        }
        Err(err) => {
            warn!(
                "Capture inner thread could not get real time priority, error: {}.",
                err
            );
            None
        }
    };
    trace!("Starting capture stream.");
    audio_client.start_stream()?;
    trace!("Started capture stream.");
    loop {
        trace!("Capturing.");
        if stop_signal.load(Ordering::Relaxed) {
            debug!("Stopping inner capture loop on request.");
            audio_client.stop_stream()?;
            return Ok(());
        }
        if handle.wait_for_event(250).is_err() {
            debug!("Timeout on capture event.");
            if !inactive {
                warn!("No data received, pausing stream.");
                inactive = true;
            }
            match channels.tx_filled.try_send((chunk_nbr, 0)) {
                Ok(()) | Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => {
                    error!("Error sending, channel disconnected.");
                    audio_client.stop_stream()?;
                    return Err(DeviceError::new("Channel disconnected.").into());
                }
            }
            chunk_nbr += 1;
            continue;
        }

        if inactive {
            info!("Data received, resuming stream.");
            inactive = false;
        }
        let available_frames = match capture_client.get_next_nbr_frames()? {
            Some(frames) => frames,
            None => audio_client.get_bufferframecount()?,
        };

        trace!("Available frames from capture dev: {}.", available_frames);

        // If no available frames, just skip the rest of this loop iteration
        if available_frames > 0 {
            let nbr_bytes = available_frames as usize * blockalign;

            let (nbr_frames_read, flags) =
                capture_client.read_from_device(&mut data[0..nbr_bytes])?;
            if nbr_frames_read != available_frames {
                warn!(
                    "Expected {} frames, got {}.",
                    available_frames, nbr_frames_read
                );
            }
            if flags.silent {
                debug!("Captured a buffer marked as silent.");
                data.iter_mut().take(nbr_bytes).for_each(|val| *val = 0);
            }
            // Disabled since VB-Audio Cable gives this all the time
            // even though there seems to be no problem. Buggy?
            //if flags.data_discontinuity {
            //    warn!("Capture device reported a buffer overrun");
            //}

            // Workaround for an issue with capturing from VB-Audio Cable
            // in shared mode. This device seems to misbehave and not provide
            // the buffers right after the event occurs.
            // Check if more samples are available and read again.
            /*
            if let Some(extra_frames) = capture_client.get_next_nbr_frames()? {
                if extra_frames > 0 {
                    trace!("Workaround, reading {} frames more.", extra_frames);
                    let nbr_bytes_extra = extra_frames as usize * blockalign;
                    let (nbr_frames_read, flags) = capture_client
                        .read_from_device(&mut data[nbr_bytes..(nbr_bytes + nbr_bytes_extra)])?;
                    if nbr_frames_read != extra_frames {
                        warn!("Expected {} frames, got {}.", extra_frames, nbr_frames_read);
                    }
                    if flags.silent {
                        debug!("Captured a buffer marked as silent.");
                        data.iter_mut()
                            .skip(nbr_bytes)
                            .take(nbr_bytes_extra)
                            .for_each(|val| *val = 0);
                    }
                    if flags.data_discontinuity {
                        warn!("Capture device reported a buffer overrun.");
                    }
                    nbr_bytes += nbr_bytes_extra;
                }
            } */
            let pushed_bytes = channels.ringbuf.push_slice(&data[0..nbr_bytes]);
            if pushed_bytes < nbr_bytes {
                debug!(
                    "Capture ring buffer is full, dropped {} out of {} bytes",
                    nbr_bytes - pushed_bytes,
                    nbr_bytes
                );
            }
            match channels.tx_filled.try_send((chunk_nbr, pushed_bytes)) {
                Ok(()) => {}
                Err(TrySendError::Full((nbr, length))) => {
                    warn!(
                        "Notification channel full, dropping chunk nbr {} with len {}.",
                        nbr, length
                    );
                }
                Err(TrySendError::Disconnected(_)) => {
                    error!("Error sending, channel disconnected.");
                    audio_client.stop_stream()?;
                    return Err(DeviceError::new("Channel disconnected.").into());
                }
            }
            chunk_nbr += 1;
        }
    }
}

/// Start a playback thread listening for AudioMessages via a channel.
impl PlaybackDevice for WasapiPlaybackDevice {
    fn start(
        &mut self,
        channel: crossbeam_channel::Receiver<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        playback_status: Arc<RwLock<PlaybackStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let devname = self.devname.clone();
        let exclusive = self.exclusive;
        let samplerate = self.samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let target_level = if self.target_level > 0 {
            self.target_level
        } else {
            self.chunksize
        };
        let adjust_period = self.adjust_period;
        let adjust = self.adjust_period > 0.0 && self.enable_rate_adjust;
        let sample_format = self.sample_format;
        let sample_format_dev = self.sample_format;
        let handle = thread::Builder::new()
            .name("WasapiPlayback".to_string())
            .spawn(move || {
                // Devices typically request around 1000 frames per buffer, set a reasonable capacity for the channel
                let channel_capacity = 8 * 1024 / chunksize + 3;
                debug!(
                    "Using a playback channel capacity of {} chunks.",
                    channel_capacity
                );
                let (tx_dev, rx_dev) = bounded(channel_capacity);
                let (tx_state_dev, rx_state_dev) = bounded(0);
                let (tx_disconnectreason, rx_disconnectreason) = unbounded();
                let buffer_fill = Arc::new(Mutex::new(countertimer::DeviceBufferEstimator::new(samplerate)));
                let buffer_fill_clone = buffer_fill.clone();
                let mut buffer_avg = countertimer::Averager::new();
                let mut timer = countertimer::Stopwatch::new();
                let mut chunk_stats = ChunkStats {
                    rms: vec![0.0; channels],
                    peak: vec![0.0; channels],
                };

                let mut rate_controller = PIRateController::new_with_default_gains(samplerate, adjust_period as f64, target_level);

                trace!("Build output stream.");
                let mut conversion_result;

                let ringbuffer = HeapRb::<u8>::new(channels * sample_format.bytes_per_sample() * ( 2 * chunksize + 2048 ));
                let (mut device_producer, device_consumer) = ringbuffer.split();

                // wasapi device loop
                let innerhandle = thread::Builder::new()
                    .name("WasapiPlaybackInner".to_string())
                    .spawn(move || {
                        let (_device, audio_client, render_client, handle, wave_format) =
                            match open_playback(
                                &devname,
                                samplerate,
                                channels,
                                &sample_format_dev,
                                exclusive,
                            ) {
                                Ok((_device, audio_client, render_client, handle, wave_format)) => {
                                    tx_state_dev.send(DeviceState::Ok).unwrap_or(());
                                    (_device, audio_client, render_client, handle, wave_format)
                                }
                                Err(err) => {
                                    let msg = format!("Playback error: {}", err);
                                    tx_state_dev.send(DeviceState::Error(msg)).unwrap_or(());
                                    return;
                                }
                            };
                        let blockalign = wave_format.get_blockalign();
                        let sync = PlaybackSync {
                            rx_play: rx_dev,
                            tx_cb: tx_disconnectreason,
                            bufferfill: buffer_fill_clone,
                        };
                        let result = playback_loop(
                            audio_client,
                            render_client,
                            handle,
                            blockalign as usize,
                            chunksize,
                            samplerate as f64,
                            sync,
                            device_consumer,
                            target_level,
                        );
                        if let Err(err) = result {
                            let msg = format!("Playback failed with error: {}", err);
                            //error!("{}", msg);
                            tx_state_dev.send(DeviceState::Error(msg)).unwrap_or(());
                        }
                    })
                    .unwrap();
                match rx_state_dev.recv() {
                    Ok(DeviceState::Ok) => {}
                    Ok(DeviceState::Error(err)) => {
                        status_channel
                            .send(StatusMessage::PlaybackError(err))
                            .unwrap_or(());
                        barrier.wait();
                        return;
                    }
                    Err(err) => {
                        status_channel
                            .send(StatusMessage::PlaybackError(err.to_string()))
                            .unwrap_or(());
                        barrier.wait();
                        return;
                    }
                }
                match status_channel.send(StatusMessage::PlaybackReady) {
                    Ok(()) => {}
                    Err(_err) => {}
                }
                debug!("Playback device ready and waiting.");
                barrier.wait();
                let thread_handle = match promote_current_thread_to_real_time(0, 1) {
                    Ok(h) => {
                        debug!("Playback outer thread has real-time priority.");
                        Some(h)
                    }
                    Err(err) => {
                        warn!(
                            "Playback outer thread could not get real time priority, error: {}.",
                            err
                        );
                        None
                    }
                };

                let mut buf =
                    vec![
                        0u8;
                        channels * chunksize * sample_format.bytes_per_sample()
                    ];

                debug!("Playback device starts now!");
                loop {
                    match rx_state_dev.try_recv() {
                        Ok(DeviceState::Ok) => {}
                        Ok(DeviceState::Error(err)) => {
                            send_error_or_playbackformatchange(
                                &status_channel,
                                &rx_disconnectreason,
                                err,
                            );
                            break;
                        }
                        Err(TryRecvError::Empty) => {}
                        Err(TryRecvError::Disconnected) => {
                            send_error_or_playbackformatchange(
                                &status_channel,
                                &rx_disconnectreason,
                                "Inner playback thread has exited".to_string(),
                            );
                            break;
                        }
                    }
                    match channel.recv() {
                        Ok(AudioMessage::Audio(chunk)) => {
                            let estimated_buffer_fill = buffer_fill.try_lock().map(|b| b.estimate() as f64).unwrap_or_default();
                            buffer_avg.add_value(estimated_buffer_fill + (channel.len() * chunksize) as f64);

                            if adjust && timer.larger_than_millis((1000.0 * adjust_period) as u64) {
                                if let Some(av_delay) = buffer_avg.average() {
                                    let speed = rate_controller.next(av_delay);
                                    timer.restart();
                                    buffer_avg.restart();
                                    debug!(
                                        "Current buffer level {:.1}, set capture rate to {:.4}%.",
                                        av_delay,
                                        100.0 * speed
                                    );
                                    status_channel
                                        .send(StatusMessage::SetSpeed(speed))
                                        .unwrap_or(());
                                    playback_status.write().buffer_level =
                                        av_delay as usize;
                                }
                            }
                            conversion_result =
                                chunk_to_buffer_rawbytes(&chunk, &mut buf, &sample_format);
                            chunk.update_stats(&mut chunk_stats);
                            if let Some(mut playback_status) = playback_status.try_write() {
                                if conversion_result.1 > 0 {
                                    playback_status.clipped_samples +=
                                        conversion_result.1;
                                }
                                playback_status
                                    .signal_rms
                                    .add_record_squared(chunk_stats.rms_linear());
                                playback_status
                                    .signal_peak
                                    .add_record(chunk_stats.peak_linear());
                            }
                            else {
                                xtrace!("Playback status blocked, skip rms update.");
                            }
                            let pushed_bytes = device_producer.push_slice(&buf[0..conversion_result.0]);
                            if pushed_bytes < conversion_result.0 {
                                debug!(
                                    "Playback ring buffer is full, dropped {} out of {} bytes",
                                    conversion_result.0 - pushed_bytes,
                                    conversion_result.0
                                );
                            }
                            match tx_dev.send(PlaybackDeviceMessage::Data(pushed_bytes)) {
                                Ok(_) => {}
                                Err(err) => {
                                    error!("Playback device channel error: {}.", err);
                                    send_error_or_playbackformatchange(
                                        &status_channel,
                                        &rx_disconnectreason,
                                        err.to_string(),
                                    );
                                    break;
                                }
                            }
                        }
                        Ok(AudioMessage::Pause) => {
                            trace!("Pause message received.");
                        }
                        Ok(AudioMessage::EndOfStream) => {
                            status_channel
                                .send(StatusMessage::PlaybackDone)
                                .unwrap_or(());
                            break;
                        }
                        Err(err) => {
                            error!("Message channel error: {}.", err);
                            send_error_or_playbackformatchange(
                                &status_channel,
                                &rx_disconnectreason,
                                err.to_string(),
                            );
                            break;
                        }
                    }
                }
                if let Some(h) = thread_handle {
                    match demote_current_thread_from_real_time(h) {
                        Ok(_) => {
                            debug!("Playback outer thread returned to normal priority.")
                        }
                        Err(_) => {
                            warn!("Could not bring the outer playback thread back to normal priority.")
                        }
                    };
                }
                match tx_dev.send(PlaybackDeviceMessage::Stop) {
                    Ok(_) => {
                        debug!("Wait for inner playback thread to exit.");
                        innerhandle.join().unwrap_or(());
                    }
                    Err(_) => {
                        warn!("Inner playback thread already stopped.")
                    }
                }
            })?;
        Ok(Box::new(handle))
    }
}

fn check_for_format_change(rx: &Receiver<DisconnectReason>) -> bool {
    loop {
        match rx.try_recv() {
            Ok(DisconnectReason::Error) => {}
            Ok(DisconnectReason::FormatChange) => {
                return true;
            }
            Err(TryRecvError::Empty) => {
                return false;
            }
            Err(TryRecvError::Disconnected) => {
                return false;
            }
        }
    }
}

fn send_error_or_playbackformatchange(
    tx: &crossbeam_channel::Sender<StatusMessage>,
    rx: &Receiver<DisconnectReason>,
    err: String,
) {
    if check_for_format_change(rx) {
        debug!("Send PlaybackFormatChange.");
        tx.send(StatusMessage::PlaybackFormatChange(0))
            .unwrap_or(());
    } else {
        debug!("Send PlaybackError.");
        tx.send(StatusMessage::PlaybackError(err)).unwrap_or(());
    }
}

fn send_error_or_captureformatchange(
    tx: &crossbeam_channel::Sender<StatusMessage>,
    rx: &Receiver<DisconnectReason>,
    err: String,
) {
    if check_for_format_change(rx) {
        debug!("Send CaptureFormatChange.");
        tx.send(StatusMessage::CaptureFormatChange(0)).unwrap_or(());
    } else {
        debug!("Send CaptureError.");
        tx.send(StatusMessage::CaptureError(err)).unwrap_or(());
    }
}

fn nbr_capture_frames(
    resampler: &Option<Box<dyn VecResampler<PrcFmt>>>,
    capture_frames: usize,
) -> usize {
    if let Some(resampl) = &resampler {
        #[cfg(feature = "debug")]
        trace!("Resampler needs {} frames.", resampl.input_frames_next());
        resampl.input_frames_next()
    } else {
        capture_frames
    }
}

/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for WasapiCaptureDevice {
    fn start(
        &mut self,
        channel: crossbeam_channel::Sender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        command_channel: crossbeam_channel::Receiver<CommandMessage>,
        capture_status: Arc<RwLock<CaptureStatus>>,
        _processing_params: Arc<ProcessingParameters>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let exclusive = self.exclusive;
        let loopback = self.loopback;
        let devname = self.devname.clone();
        let samplerate = self.samplerate;
        let capture_samplerate = self.capture_samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let bytes_per_sample = self.sample_format.bytes_per_sample();
        let sample_format = self.sample_format;
        let sample_format_dev = self.sample_format;
        let resampler_conf = self.resampler_config;
        let async_src = resampler_is_async(&resampler_conf);
        let silence_timeout = self.silence_timeout;
        let silence_threshold = self.silence_threshold;
        let stop_on_rate_change = self.stop_on_rate_change;
        let rate_measure_interval = (1000.0 * self.rate_measure_interval) as u64;
        let handle = thread::Builder::new()
            .name("WasapiCapture".to_string())
            .spawn(move || {
                let mut resampler = new_resampler(
                        &resampler_conf,
                        channels,
                        samplerate,
                        capture_samplerate,
                        chunksize,
                );
                // Devices typically give around 1000 frames per buffer, set a reasonable capacity for the channel
                let channel_capacity = 16*chunksize/1024 + 10;
                debug!("Using a capture channel capacity of {} buffers.", channel_capacity);
                let (tx_dev, rx_dev) = bounded(channel_capacity);

                let (tx_state_dev, rx_state_dev) = bounded(0);
                let (tx_start_inner, rx_start_inner) = bounded(0);
                let (tx_disconnectreason, rx_disconnectreason) = unbounded();

                let ringbuffer = HeapRb::<u8>::new(channels * bytes_per_sample * ( 2 * chunksize + 2048 ));
                let (device_producer, mut device_consumer) = ringbuffer.split();

                trace!("Build input stream.");
                // wasapi device loop
                let stop_signal = Arc::new(AtomicBool::new(false));
                let stop_signal_inner = stop_signal.clone();
                let innerhandle = thread::Builder::new()
                    .name("WasapiCaptureInner".to_string())
                    .spawn(move || {
                        let (_device, audio_client, capture_client, handle, wave_format) =
                        match open_capture(&devname, capture_samplerate, channels, &sample_format_dev, exclusive, loopback) {
                            Ok((_device, audio_client, capture_client, handle, wave_format)) => {
                                tx_state_dev.send(DeviceState::Ok).unwrap_or(());
                                (_device, audio_client, capture_client, handle, wave_format)
                            },
                            Err(err) => {
                                error!("Failed to open capture device, error: {}", err);
                                let msg = format!("Capture error: {}", err);
                                tx_state_dev.send(DeviceState::Error(msg)).unwrap_or(());
                                return;
                            }
                        };
                        let blockalign = wave_format.get_blockalign();
                        let channels =  CaptureChannels {
                            tx_filled: tx_dev,
                            ringbuf: device_producer,
                        };
                        let _rx_res = rx_start_inner.recv();
                        let result = capture_loop(audio_client, capture_client, handle, channels, tx_disconnectreason, blockalign as usize, stop_signal_inner);
                        if let Err(err) = result {
                            let msg = format!("Capture failed with error: {}", err);
                            //error!("{}", msg);
                            tx_state_dev.send(DeviceState::Error(msg)).unwrap_or(());
                        }
                    }).unwrap();
                match rx_state_dev.recv() {
                    Ok(DeviceState::Ok) => {},
                    Ok(DeviceState::Error(err)) => {
                        channel.send(AudioMessage::EndOfStream).unwrap_or(());
                        status_channel.send(StatusMessage::CaptureError(err)).unwrap_or(());
                        barrier.wait();
                        return;
                     },
                    Err(err) => {
                        channel.send(AudioMessage::EndOfStream).unwrap_or(());
                        status_channel.send(StatusMessage::CaptureError(err.to_string() )).unwrap_or(());
                        barrier.wait();
                        return;
                    },
                }
                let mut capture_frames = chunksize;
                let mut averager = countertimer::TimeAverage::new();
                let mut watcher_averager = countertimer::TimeAverage::new();
                let mut valuewatcher = countertimer::ValueWatcher::new(capture_samplerate as f32, RATE_CHANGE_THRESHOLD_VALUE, RATE_CHANGE_THRESHOLD_COUNT);
                let mut value_range = 0.0;
                let mut chunk_stats = ChunkStats{rms: vec![0.0; channels], peak: vec![0.0; channels]};
                let mut rate_adjust = 0.0;
                let mut silence_counter = countertimer::SilenceCounter::new(silence_threshold, silence_timeout, capture_samplerate, chunksize);
                let mut state = ProcessingState::Running;
                let mut saved_state = state;
                let blockalign = bytes_per_sample*channels;
                // TODO check if this ever needs to be resized
                let mut data_buffer = vec![0u8; 4 * blockalign * capture_frames];
                let mut expected_chunk_nbr = 0;
                let mut channel_mask = vec![true; channels];
                debug!("Capture device ready and waiting.");
                match status_channel.send(StatusMessage::CaptureReady) {
                    Ok(()) => {}
                    Err(_err) => {}
                }
                barrier.wait();
                let thread_handle = match promote_current_thread_to_real_time(0, 1) {
                    Ok(h) => {
                        debug!("Capture outer thread has real-time priority.");
                        Some(h)
                    }
                    Err(err) => {
                        warn!(
                            "Capture outer thread could not get real time priority, error: {}.",
                            err
                        );
                        None
                    }
                };
                let _send_res = tx_start_inner.send(());
                debug!("Capture device starts now!");
                loop {
                    match command_channel.try_recv() {
                        Ok(CommandMessage::Exit) => {
                            debug!("Exit message received, sending EndOfStream.");
                            let msg = AudioMessage::EndOfStream;
                            channel.send(msg).unwrap_or(());
                            status_channel.send(StatusMessage::CaptureDone).unwrap_or(());
                            break;
                        }
                        Ok(CommandMessage::SetSpeed { speed }) => {
                            rate_adjust = speed;
                            debug!("Requested to adjust capture speed to {}.", speed);
                            if let Some(resampl) = &mut resampler {
                                debug!("Adjusting resampler rate to {}.", speed);
                                if async_src {
                                    if resampl.set_resample_ratio_relative(speed, true).is_err() {
                                        debug!("Failed to set resampling speed to {}.", speed);
                                    }
                                }
                                else {
                                    warn!("Requested rate adjust of synchronous resampler. Ignoring request.");
                                }
                            }
                        },
                        Err(crossbeam_channel::TryRecvError::Empty) => {}
                        Err(crossbeam_channel::TryRecvError::Disconnected) => {
                            error!("Command channel was closed.");
                            break;
                        }
                    };
                    match rx_state_dev.try_recv() {
                        Ok(DeviceState::Ok) => {},
                        Ok(DeviceState::Error(err)) => {
                            channel.send(AudioMessage::EndOfStream).unwrap_or(());
                            send_error_or_captureformatchange(&status_channel, &rx_disconnectreason, err);
                            break;
                        },
                        Err(TryRecvError::Empty) => {}
                        Err(TryRecvError::Disconnected) => {
                            channel.send(AudioMessage::EndOfStream).unwrap_or(());
                            send_error_or_captureformatchange(&status_channel, &rx_disconnectreason, "Inner capture thread exited.".to_string());
                            break;
                        }
                    }
                    capture_frames = nbr_capture_frames(
                        &resampler,
                        capture_frames,
                    );
                    let capture_bytes = blockalign * capture_frames;
                    while device_consumer.occupied_len() < (blockalign * capture_frames) {
                        trace!("Capture device needs more samples to make chunk, reading from channel.");
                        match rx_dev.recv() {
                            Ok((chunk_nbr, data_bytes)) => {
                                trace!("Received chunk, length {} bytes.", data_bytes);
                                expected_chunk_nbr += 1;
                                if data_bytes == 0 {
                                    if state != ProcessingState::Stalled {
                                        trace!("Capture device became inactive.");
                                        saved_state = state;
                                        state = ProcessingState::Stalled;
                                    }
                                    break;
                                }
                                else if state == ProcessingState::Stalled {
                                    trace!("Capture device became active.");
                                    state = saved_state;
                                }
                                if chunk_nbr > expected_chunk_nbr {
                                    warn!("Samples were dropped, missing {} buffers.", chunk_nbr - expected_chunk_nbr);
                                    expected_chunk_nbr = chunk_nbr;
                                }
                            }
                            Err(err) => {
                                error!("Channel is closed.");
                                channel.send(AudioMessage::EndOfStream).unwrap_or(());
                                send_error_or_captureformatchange(&status_channel, &rx_disconnectreason, err.to_string());
                                return;
                            }
                        }
                    }
                    if state != ProcessingState::Stalled {
                        device_consumer.pop_slice(&mut data_buffer[0..capture_bytes]);
                        averager.add_value(capture_frames);
                        if let Some(capture_status) = capture_status.try_upgradable_read() {
                            if averager.larger_than_millis(capture_status.update_interval as u64)
                            {
                                let samples_per_sec = averager.average();
                                averager.restart();
                                let measured_rate_f = samples_per_sec;
                                if let Ok(mut capture_status) = RwLockUpgradableReadGuard::try_upgrade(capture_status) {
                                    capture_status.measured_samplerate = measured_rate_f as usize;
                                    capture_status.signal_range = value_range as f32;
                                    capture_status.rate_adjust = rate_adjust as f32;
                                    capture_status.state = state;
                                }
                                else {
                                    xtrace!("Capture status upgrade blocked, skip update.");
                                }
                            }
                        }
                        else {
                            xtrace!("Capture status blocked, skip update.");
                        }
                        watcher_averager.add_value(capture_frames);
                        if watcher_averager.larger_than_millis(rate_measure_interval)
                        {
                            let samples_per_sec = watcher_averager.average();
                            watcher_averager.restart();
                            let measured_rate_f = samples_per_sec;
                            debug!(
                                "Measured sample rate is {:.1} Hz.",
                                measured_rate_f
                            );
                            let changed = valuewatcher.check_value(measured_rate_f as f32);
                            if changed {
                                warn!("Sample rate change detected, last rate was {} Hz.", measured_rate_f);
                                if stop_on_rate_change {
                                    let msg = AudioMessage::EndOfStream;
                                    channel.send(msg).unwrap_or(());
                                    status_channel.send(StatusMessage::CaptureFormatChange(measured_rate_f as usize)).unwrap_or(());
                                    break;
                                }
                            }
                        }
                        let mut chunk = buffer_to_chunk_rawbytes(
                            &data_buffer[0..capture_bytes],
                            channels,
                            &sample_format,
                            capture_bytes,
                            &capture_status.read().used_channels,
                        );
                        chunk.update_stats(&mut chunk_stats);
                        if let Some(mut capture_status) = capture_status.try_write() {
                            capture_status.signal_rms.add_record_squared(chunk_stats.rms_linear());
                            capture_status.signal_peak.add_record(chunk_stats.peak_linear());
                        }
                        else {
                            xtrace!("Capture status blocked, skip rms update.");
                        }
                        value_range = chunk.maxval - chunk.minval;
                        state = silence_counter.update(value_range);
                        if state == ProcessingState::Running {
                            if let Some(resampl) = &mut resampler {
                                chunk.update_channel_mask(&mut channel_mask);
                                let new_waves = resampl.process(&chunk.waveforms, Some(&channel_mask)).unwrap();
                                let mut chunk_frames = new_waves.iter().map(|w| w.len()).max().unwrap();
                                if chunk_frames == 0 {
                                    chunk_frames = chunksize;
                                }
                                chunk.frames = chunk_frames;
                                chunk.valid_frames = chunk.frames;
                                chunk.waveforms = new_waves;
                            }
                            let msg = AudioMessage::Audio(chunk);
                            if channel.send(msg).is_err() {
                                info!("Processing thread has already stopped.");
                                break;
                            }
                        }
                    }
                    if state == ProcessingState::Paused || state == ProcessingState::Stalled {
                        let msg = AudioMessage::Pause;
                        if channel.send(msg).is_err() {
                            info!("Processing thread has already stopped.");
                            break;
                        }
                    }
                }
                if let Some(h) = thread_handle {
                    match demote_current_thread_from_real_time(h) {
                        Ok(_) => {
                            debug!("Capture outer thread returned to normal priority.")
                        }
                        Err(_) => {
                            warn!("Could not bring the outer capture thread back to normal priority.")
                        }
                    };
                }
                stop_signal.store(true, Ordering::Relaxed);
                debug!("Wait for inner capture thread to exit.");
                innerhandle.join().unwrap_or(());
                capture_status.write().state = ProcessingState::Inactive;
            })?;
        Ok(Box::new(handle))
    }
}
