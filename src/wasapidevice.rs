use crate::audiodevice::*;
use crate::config;
use crate::config::{ConfigError, SampleFormat};
use crate::conversions::{buffer_to_chunk_rawbytes, chunk_to_buffer_rawbytes};
use crate::countertimer;
use crossbeam_channel::{bounded, unbounded, Receiver, Sender, TryRecvError, TrySendError};
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use rubato::VecResampler;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;
use wasapi;
use wasapi::DeviceCollection;
use windows::w;
use windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW;

use crate::CommandMessage;
use crate::PrcFmt;
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
            "Available playback devices: {:?}",
            list_device_names_in_collection(&collection)
        );
        collection.get_device_with_name(name)?
    } else {
        wasapi::get_default_device(&wasapi::Direction::Render)?
    };
    trace!("Found playback device {:?}", devname);
    let mut audio_client = device.get_iaudioclient()?;
    trace!("Got playback iaudioclient");
    let wave_format = wave_format(sample_format, samplerate, channels);
    match audio_client.is_supported(&wave_format, &sharemode) {
        Ok(None) => {
            debug!("Playback device supports format {:?}", wave_format)
        }
        Ok(Some(modified)) => {
            let msg = format!(
                "Playback device doesn't support format:\n{:#?}\nClosest match is:\n{:#?}",
                wave_format, modified
            );
            return Err(ConfigError::new(&msg).into());
        }
        Err(err) => {
            let msg = format!(
                "Playback device doesn't support format:\n{:#?}\nError: {}",
                wave_format, err
            );
            return Err(ConfigError::new(&msg).into());
        }
    };
    let (def_time, min_time) = audio_client.get_periods()?;
    debug!(
        "playback default period {}, min period {}",
        def_time, min_time
    );
    audio_client.initialize_client(
        &wave_format,
        def_time,
        &wasapi::Direction::Render,
        &sharemode,
        false,
    )?;
    debug!("initialized capture");
    let handle = audio_client.set_get_eventhandle()?;
    let render_client = audio_client.get_audiorenderclient()?;
    debug!("Opened Wasapi playback device {:?}", devname);
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
                "Available capture devices: {:?}",
                list_device_names_in_collection(&collection)
            );
            collection.get_device_with_name(name)?
        } else {
            let collection = wasapi::DeviceCollection::new(&wasapi::Direction::Render)?;
            debug!(
                "Available loopback capture (i.e. playback) devices: {:?}",
                list_device_names_in_collection(&collection)
            );
            collection.get_device_with_name(name)?
        }
    } else if !loopback {
        wasapi::get_default_device(&wasapi::Direction::Capture)?
    } else {
        wasapi::get_default_device(&wasapi::Direction::Render)?
    };

    trace!("Found capture device {:?}", devname);
    let mut audio_client = device.get_iaudioclient()?;
    trace!("Got capture iaudioclient");
    let wave_format = wave_format(sample_format, samplerate, channels);
    match audio_client.is_supported(&wave_format, &sharemode) {
        Ok(None) => {
            debug!("Capture device supports format {:?}", wave_format)
        }
        Ok(Some(modified)) => {
            let msg = format!(
                "Capture device doesn't support format:\n{:#?}\nClosest match is:\n{:#?}",
                wave_format, modified
            );
            return Err(ConfigError::new(&msg).into());
        }
        Err(err) => {
            let msg = format!(
                "Capture device doesn't support format:\n{:#?}\nError: {}",
                wave_format, err
            );
            return Err(ConfigError::new(&msg).into());
        }
    };
    let (def_time, min_time) = audio_client.get_periods()?;
    debug!(
        "capture default period {}, min period {}",
        def_time, min_time
    );
    audio_client.initialize_client(
        &wave_format,
        def_time,
        &wasapi::Direction::Capture,
        &sharemode,
        loopback,
    )?;
    debug!("initialized capture");
    let handle = audio_client.set_get_eventhandle()?;
    trace!("capture got event handle");
    let capture_client = audio_client.get_audiocaptureclient()?;
    debug!("Opened Wasapi capture device {:?}", devname);
    Ok((device, audio_client, capture_client, handle, wave_format))
}

struct PlaybackSync {
    rx_play: Receiver<PlaybackDeviceMessage>,
    tx_cb: Sender<DisconnectReason>,
    bufferfill: Arc<AtomicUsize>,
}

enum PlaybackDeviceMessage {
    Data(Vec<u8>),
    Stop,
}

// Playback loop, play samples received from channel
fn playback_loop(
    audio_client: wasapi::AudioClient,
    render_client: wasapi::AudioRenderClient,
    handle: wasapi::Handle,
    blockalign: usize,
    chunksize: usize,
    samplerate: f64,
    sync: PlaybackSync,
) -> Res<()> {
    let mut buffer_free_frame_count = audio_client.get_bufferframecount()?;
    let mut sample_queue: VecDeque<u8> = VecDeque::with_capacity(
        4 * blockalign * (chunksize + 2 * buffer_free_frame_count as usize),
    );

    let tx_cb = sync.tx_cb;
    let mut callbacks = wasapi::EventCallbacks::new();
    callbacks.set_disconnected_callback(move |reason| {
        debug!("Disconnected, reason: {:?}", reason);
        let simplereason = match reason {
            wasapi::DisconnectReason::FormatChanged => DisconnectReason::FormatChange,
            _ => DisconnectReason::Error,
        };
        tx_cb.send(simplereason).unwrap_or(());
    });
    let callbacks_rc = Rc::new(callbacks);
    let callbacks_weak = Rc::downgrade(&callbacks_rc);
    let sessioncontrol = audio_client.get_audiosessioncontrol()?;
    let clock = audio_client.get_audioclock()?;
    sessioncontrol.register_session_notification(callbacks_weak)?;

    let mut waited_millis = 0;
    trace!("Waiting for data to start playback, will time out after one second");
    while sync.rx_play.len() < 2 && waited_millis < 1000 {
        thread::sleep(Duration::from_millis(10));
        waited_millis += 10;
    }
    debug!("Waited for data for {} ms", waited_millis);

    // Raise priority
    let mut task_idx = 0;
    unsafe {
        let _ = AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task_idx);
    }
    if task_idx > 0 {
        debug!("Playback thread raised priority, task index: {}", task_idx);
    } else {
        warn!("Failed to raise playback thread priority");
    }

    audio_client.start_stream()?;
    let mut running = true;
    let mut pos = 0;
    let mut device_prevtime = 0.0;
    let device_freq = clock.get_frequency()? as f64;
    loop {
        buffer_free_frame_count = audio_client.get_available_space_in_frames()?;
        trace!("New buffer frame count {}", buffer_free_frame_count);
        let mut device_time = pos as f64 / device_freq;
        if device_time == 0.0 && device_prevtime > 0.0 {
            debug!("Failed to get accurate device time, skipping check for missing events");
            // A zero value means that the device position read was delayed due to some
            // other high priority event, and an accurate reading could not be taken.
            // To avoid needless resets of the stream, set the position to the expected value,
            // calculated as the previous value plus the expected increment.
            device_time = device_prevtime + buffer_free_frame_count as f64 / samplerate;
        }
        trace!(
            "Device time counted up by {} s",
            device_time - device_prevtime
        );
        if buffer_free_frame_count > 0
            && (device_time - device_prevtime) > 1.5 * (buffer_free_frame_count as f64 / samplerate)
        {
            warn!(
                "Missing event! Resetting stream. Interval {} s, expected {} s",
                device_time - device_prevtime,
                buffer_free_frame_count as f64 / samplerate
            );
            audio_client.stop_stream()?;
            audio_client.reset_stream()?;
            audio_client.start_stream()?;
        }
        device_prevtime = device_time;

        while sample_queue.len() < (blockalign * buffer_free_frame_count as usize) {
            trace!("playback loop needs more samples, reading from channel");
            match sync.rx_play.try_recv() {
                Ok(PlaybackDeviceMessage::Data(chunk)) => {
                    trace!("got chunk");
                    for element in chunk.iter() {
                        sample_queue.push_back(*element);
                    }
                    if !running {
                        running = true;
                        info!("Restarting playback after buffer underrun");
                    }
                }
                Ok(PlaybackDeviceMessage::Stop) => {
                    debug!("Stopping inner playback loop");
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
                        warn!("Playback interrupted, no data available");
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    error!("Channel is closed");
                    return Err(DeviceError::new("Data channel closed").into());
                }
            }
        }
        render_client.write_to_device_from_deque(
            buffer_free_frame_count as usize,
            blockalign,
            &mut sample_queue,
            None,
        )?;
        let curr_buffer_fill = sample_queue.len() / blockalign + sync.rx_play.len() * chunksize;
        sync.bufferfill.store(curr_buffer_fill, Ordering::Relaxed);
        trace!("write ok");
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
    pub tx_filled: Sender<(u64, usize, Vec<u8>)>,
    pub rx_empty: Receiver<Vec<u8>>,
}

// Capture loop, capture samples and send in chunks of "chunksize" frames to channel
fn capture_loop(
    audio_client: wasapi::AudioClient,
    capture_client: wasapi::AudioCaptureClient,
    handle: wasapi::Handle,
    channels: CaptureChannels,
    tx_disconnectreason: Sender<DisconnectReason>,
    blockalign: usize,
    stop_signal: Arc<AtomicBool>,
) -> Res<()> {
    let mut chunk_nbr: u64 = 0;

    let mut callbacks = wasapi::EventCallbacks::new();
    callbacks.set_disconnected_callback(move |reason| {
        debug!("Capture disconnected, reason: {:?}", reason);
        let simplereason = match reason {
            wasapi::DisconnectReason::FormatChanged => DisconnectReason::FormatChange,
            _ => DisconnectReason::Error,
        };
        tx_disconnectreason.send(simplereason).unwrap_or(());
    });

    let callbacks_rc = Rc::new(callbacks);
    let callbacks_weak = Rc::downgrade(&callbacks_rc);

    let sessioncontrol = audio_client.get_audiosessioncontrol()?;
    sessioncontrol.register_session_notification(callbacks_weak)?;

    let mut inactive = false;

    let mut saved_buffer: Option<Vec<u8>> = None;

    // Raise priority
    let mut task_idx = 0;
    unsafe {
        let _ = AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task_idx);
    }
    if task_idx > 0 {
        debug!("Capture thread raised priority, task index: {}", task_idx);
    } else {
        warn!("Failed to raise capture thread priority");
    }
    trace!("Starting capture stream");
    audio_client.start_stream()?;
    trace!("Started capture stream");
    loop {
        trace!("capturing");
        if stop_signal.load(Ordering::Relaxed) {
            debug!("Stopping inner capture loop on request");
            audio_client.stop_stream()?;
            return Ok(());
        }
        if handle.wait_for_event(250).is_err() {
            debug!("Timeout on capture event");
            if !inactive {
                warn!("No data received, pausing stream");
                inactive = true;
            }
            let data = vec![0u8; 0];
            match channels.tx_filled.try_send((chunk_nbr, 0, data)) {
                Ok(()) | Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => {
                    error!("Error sending, channel disconnected");
                    audio_client.stop_stream()?;
                    return Err(DeviceError::new("Channel disconnected").into());
                }
            }
            chunk_nbr += 1;
            continue;
        }

        if inactive {
            info!("Data received, resuming stream");
            inactive = false;
        }
        let available_frames = match capture_client.get_next_nbr_frames()? {
            Some(frames) => frames,
            None => audio_client.get_bufferframecount()?,
        };

        trace!("Available frames from capture dev: {}", available_frames);

        // If no available frames, just skip the rest of this loop iteration
        if available_frames > 0 {
            //let mut data = vec![0u8; available_frames as usize * blockalign as usize];
            let mut data = match saved_buffer {
                Some(buf) => {
                    saved_buffer = None;
                    buf
                }
                None => channels.rx_empty.recv().unwrap(),
            };

            let mut nbr_bytes = available_frames as usize * blockalign;
            if data.len() < nbr_bytes {
                data.resize(nbr_bytes, 0);
            }
            let (nbr_frames_read, flags) =
                capture_client.read_from_device(blockalign, &mut data[0..nbr_bytes])?;
            if nbr_frames_read != available_frames {
                warn!(
                    "Expected {} frames, got {}",
                    available_frames, nbr_frames_read
                );
            }
            if flags.silent {
                debug!("Captured a buffer marked as silent");
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
            if let Some(extra_frames) = capture_client.get_next_nbr_frames()? {
                if extra_frames > 0 {
                    trace!("Workaround, reading {} frames more", extra_frames);
                    let nbr_bytes_extra = extra_frames as usize * blockalign;
                    if data.len() < (nbr_bytes + nbr_bytes_extra) {
                        data.resize(nbr_bytes + nbr_bytes_extra, 0);
                    }
                    let (nbr_frames_read, flags) = capture_client.read_from_device(
                        blockalign,
                        &mut data[nbr_bytes..(nbr_bytes + nbr_bytes_extra)],
                    )?;
                    if nbr_frames_read != extra_frames {
                        warn!("Expected {} frames, got {}", extra_frames, nbr_frames_read);
                    }
                    if flags.silent {
                        debug!("Captured a buffer marked as silent");
                        data.iter_mut()
                            .skip(nbr_bytes)
                            .take(nbr_bytes_extra)
                            .for_each(|val| *val = 0);
                    }
                    if flags.data_discontinuity {
                        warn!("Capture device reported a buffer overrun");
                    }
                    nbr_bytes += nbr_bytes_extra;
                }
            }

            match channels.tx_filled.try_send((chunk_nbr, nbr_bytes, data)) {
                Ok(()) => {}
                Err(TrySendError::Full((nbr, length, data))) => {
                    debug!("Dropping captured chunk {} with len {}", nbr, length);
                    saved_buffer = Some(data);
                }
                Err(TrySendError::Disconnected(_)) => {
                    error!("Error sending, channel disconnected");
                    audio_client.stop_stream()?;
                    return Err(DeviceError::new("Channel disconnected").into());
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
        channel: mpsc::Receiver<AudioMessage>,
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
                let channel_capacity = 8 * 1024 / chunksize + 1;
                debug!(
                    "Using a playback channel capacity of {} chunks.",
                    channel_capacity
                );
                let (tx_dev, rx_dev) = bounded(channel_capacity);
                let (tx_state_dev, rx_state_dev) = bounded(0);
                let (tx_disconnectreason, rx_disconnectreason) = unbounded();
                let buffer_fill = Arc::new(AtomicUsize::new(0));
                let buffer_fill_clone = buffer_fill.clone();
                let mut buffer_avg = countertimer::Averager::new();
                let mut timer = countertimer::Stopwatch::new();
                let mut chunk_stats = ChunkStats {
                    rms: vec![0.0; channels],
                    peak: vec![0.0; channels],
                };

                trace!("Build output stream");
                let mut conversion_result;

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
                debug!("Playback device ready and waiting");
                barrier.wait();
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
                            let mut buf =
                                vec![
                                    0u8;
                                    channels * chunk.frames * sample_format.bytes_per_sample()
                                ];
                            buffer_avg.add_value(buffer_fill.load(Ordering::Relaxed) as f64);
                            {
                                if adjust && timer.larger_than_millis((1000.0 * adjust_period) as u64) {
                                    if let Some(av_delay) = buffer_avg.average() {
                                        let speed = calculate_speed(
                                            av_delay,
                                            target_level,
                                            adjust_period,
                                            samplerate as u32,
                                        );
                                        timer.restart();
                                        buffer_avg.restart();
                                        debug!(
                                            "Current buffer level {:.1}, set capture rate to {:.4}%",
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
                                {
                                    let mut playback_status = playback_status.write();
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
                            }
                            match tx_dev.send(PlaybackDeviceMessage::Data(buf)) {
                                Ok(_) => {}
                                Err(err) => {
                                    error!("Playback device channel error: {}", err);
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
                            trace!("Pause message received");
                        }
                        Ok(AudioMessage::EndOfStream) => {
                            status_channel
                                .send(StatusMessage::PlaybackDone)
                                .unwrap_or(());
                            break;
                        }
                        Err(err) => {
                            error!("Message channel error: {}", err);
                            send_error_or_playbackformatchange(
                                &status_channel,
                                &rx_disconnectreason,
                                err.to_string(),
                            );
                            break;
                        }
                    }
                }
                match tx_dev.send(PlaybackDeviceMessage::Stop) {
                    Ok(_) => {
                        debug!("Wait for inner playback thread to exit");
                        innerhandle.join().unwrap_or(());
                    }
                    Err(_) => {
                        warn!("Inner playback thread already stopped")
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
        debug!("Send PlaybackFormatChange");
        tx.send(StatusMessage::PlaybackFormatChange(0))
            .unwrap_or(());
    } else {
        debug!("Send PlaybackError");
        tx.send(StatusMessage::PlaybackError(err)).unwrap_or(());
    }
}

fn send_error_or_captureformatchange(
    tx: &crossbeam_channel::Sender<StatusMessage>,
    rx: &Receiver<DisconnectReason>,
    err: String,
) {
    if check_for_format_change(rx) {
        debug!("Send CaptureFormatChange");
        tx.send(StatusMessage::CaptureFormatChange(0)).unwrap_or(());
    } else {
        debug!("Send CaptureError");
        tx.send(StatusMessage::CaptureError(err)).unwrap_or(());
    }
}

fn nbr_capture_frames(
    resampler: &Option<Box<dyn VecResampler<PrcFmt>>>,
    capture_frames: usize,
) -> usize {
    if let Some(resampl) = &resampler {
        #[cfg(feature = "debug")]
        trace!("Resampler needs {} frames", resampl.input_frames_next());
        resampl.input_frames_next()
    } else {
        capture_frames
    }
}

/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for WasapiCaptureDevice {
    fn start(
        &mut self,
        channel: mpsc::SyncSender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        command_channel: mpsc::Receiver<CommandMessage>,
        capture_status: Arc<RwLock<CaptureStatus>>,
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
                let channel_capacity = 8*chunksize/1024 + 1;
                debug!("Using a capture channel capacity of {} buffers.", channel_capacity);
                let (tx_dev, rx_dev) = bounded(channel_capacity);
                let (tx_dev_free, rx_dev_free) = bounded(channel_capacity+2);
                for _ in 0..(channel_capacity+2) {
                    let data = vec![0u8; 2*1024*bytes_per_sample*channels];
                    tx_dev_free.send(data).unwrap();
                }
                let (tx_state_dev, rx_state_dev) = bounded(0);
                let (tx_disconnectreason, rx_disconnectreason) = unbounded();

                trace!("Build input stream");
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
                            rx_empty: rx_dev_free,
                        };
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
                let chunksize_samples = channels * chunksize;
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
                let mut data_queue: VecDeque<u8> = VecDeque::with_capacity(4 * blockalign * chunksize_samples );
                // TODO check if this ever needs to be resized
                let mut data_buffer = vec![0u8; 4 * blockalign * capture_frames];
                let mut expected_chunk_nbr = 0;
                let mut channel_mask = vec![true; channels];
                debug!("Capture device ready and waiting");
                match status_channel.send(StatusMessage::CaptureReady) {
                    Ok(()) => {}
                    Err(_err) => {}
                }
                barrier.wait();
                debug!("Capture device starts now!");
                loop {
                    match command_channel.try_recv() {
                        Ok(CommandMessage::Exit) => {
                            debug!("Exit message received, sending EndOfStream");
                            let msg = AudioMessage::EndOfStream;
                            channel.send(msg).unwrap_or(());
                            status_channel.send(StatusMessage::CaptureDone).unwrap_or(());
                            break;
                        }
                        Ok(CommandMessage::SetSpeed { speed }) => {
                            rate_adjust = speed;
                            debug!("Requested to adjust capture speed to {}", speed);
                            if let Some(resampl) = &mut resampler {
                                debug!("Adjusting resampler rate to {}", speed);
                                if async_src {
                                    if resampl.set_resample_ratio_relative(speed, true).is_err() {
                                        debug!("Failed to set resampling speed to {}", speed);
                                    }
                                }
                                else {
                                    warn!("Requested rate adjust of synchronous resampler. Ignoring request.");
                                }
                            }
                        },
                        Err(mpsc::TryRecvError::Empty) => {}
                        Err(mpsc::TryRecvError::Disconnected) => {
                            error!("Command channel was closed");
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
                            send_error_or_captureformatchange(&status_channel, &rx_disconnectreason, "Inner capture thread exited".to_string());
                            break;
                        }
                    }
                    capture_frames = nbr_capture_frames(
                        &resampler,
                        capture_frames,
                    );
                    let capture_bytes = blockalign * capture_frames;
                    while data_queue.len() < (blockalign * capture_frames) {
                        trace!("capture device needs more samples to make chunk, reading from channel");
                        match rx_dev.recv() {
                            Ok((chunk_nbr, data_bytes, data)) => {
                                trace!("got chunk, length {} bytes", data_bytes);
                                expected_chunk_nbr += 1;
                                if data_bytes == 0 {
                                    if state != ProcessingState::Stalled {
                                        trace!("capture device became inactive");
                                        saved_state = state;
                                        state = ProcessingState::Stalled;
                                    }
                                    break;
                                }
                                else if state == ProcessingState::Stalled {
                                    trace!("capture device became active");
                                    state = saved_state;
                                }
                                if chunk_nbr > expected_chunk_nbr {
                                    warn!("Samples were dropped, missing {} buffers", chunk_nbr - expected_chunk_nbr);
                                    expected_chunk_nbr = chunk_nbr;
                                }
                                for element in data.iter().take(data_bytes) {
                                    data_queue.push_back(*element);
                                }
                                // Return the buffer to the queue
                                tx_dev_free.send(data).unwrap();
                            }
                            Err(err) => {
                                error!("Channel is closed");
                                channel.send(AudioMessage::EndOfStream).unwrap_or(());
                                send_error_or_captureformatchange(&status_channel, &rx_disconnectreason, err.to_string());
                                return;
                            }
                        }
                    }
                    if state != ProcessingState::Stalled {
                        for element in data_buffer.iter_mut().take(capture_bytes) {
                            *element = data_queue.pop_front().unwrap();
                        }
                        averager.add_value(capture_frames);
                        {
                            let capture_status = capture_status.upgradable_read();
                            if averager.larger_than_millis(capture_status.update_interval as u64)
                            {
                                let samples_per_sec = averager.average();
                                averager.restart();
                                let measured_rate_f = samples_per_sec;
                                debug!(
                                    "Measured sample rate is {:.1} Hz",
                                    measured_rate_f
                                );
                                let mut capture_status = RwLockUpgradableReadGuard::upgrade(capture_status); // to write lock
                                capture_status.measured_samplerate = measured_rate_f as usize;
                                capture_status.signal_range = value_range as f32;
                                capture_status.rate_adjust = rate_adjust as f32;
                                capture_status.state = state;
                            }
                        }
                        watcher_averager.add_value(capture_frames);
                        if watcher_averager.larger_than_millis(rate_measure_interval)
                        {
                            let samples_per_sec = watcher_averager.average();
                            watcher_averager.restart();
                            let measured_rate_f = samples_per_sec;
                            debug!(
                                "Measured sample rate is {:.1} Hz",
                                measured_rate_f
                            );
                            let changed = valuewatcher.check_value(measured_rate_f as f32);
                            if changed {
                                warn!("sample rate change detected, last rate was {} Hz", measured_rate_f);
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
                        //trace!("Capture rms {:?}, peak {:?}", chunk_stats.rms_db(), chunk_stats.peak_db());
                        {
                            let mut capture_status = capture_status.write();
                            capture_status.signal_rms.add_record_squared(chunk_stats.rms_linear());
                            capture_status.signal_peak.add_record(chunk_stats.peak_linear());
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
                stop_signal.store(true, Ordering::Relaxed);
                debug!("Wait for inner capture thread to exit");
                innerhandle.join().unwrap_or(());
                capture_status.write().state = ProcessingState::Inactive;
            })?;
        Ok(Box::new(handle))
    }
}
