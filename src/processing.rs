use crate::audiodevice::*;
use crate::config;
use crate::filters;
use crate::ProcessingParameters;
use audio_thread_priority::{
    demote_current_thread_from_real_time, promote_current_thread_to_real_time,
};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::thread;

pub fn run_processing(
    conf_proc: config::Configuration,
    barrier_proc: Arc<Barrier>,
    tx_pb: mpsc::SyncSender<AudioMessage>,
    rx_cap: mpsc::Receiver<AudioMessage>,
    rx_pipeconf: mpsc::Receiver<(config::ConfigChange, config::Configuration)>,
    processing_params: Arc<ProcessingParameters>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let chunksize = conf_proc.devices.chunksize;
        let samplerate = conf_proc.devices.samplerate;
        let mut pipeline = filters::Pipeline::from_config(conf_proc, processing_params.clone());
        debug!("build filters, waiting to start processing loop");

        let thread_handle =
            match promote_current_thread_to_real_time(chunksize as u32, samplerate as u32) {
                Ok(h) => {
                    debug!("Processing thread has real-time priority.");
                    Some(h)
                }
                Err(err) => {
                    warn!(
                        "Processing thread could not get real time priority, error: {}",
                        err
                    );
                    None
                }
            };

        barrier_proc.wait();
        debug!("Processing loop starts now!");
        loop {
            match rx_cap.recv() {
                Ok(AudioMessage::Audio(mut chunk)) => {
                    //trace!("AudioMessage::Audio received");
                    chunk = pipeline.process_chunk(chunk);
                    let msg = AudioMessage::Audio(chunk);
                    if tx_pb.send(msg).is_err() {
                        info!("Playback thread has already stopped.");
                        break;
                    }
                }
                Ok(AudioMessage::EndOfStream) => {
                    trace!("AudioMessage::EndOfStream received");
                    let msg = AudioMessage::EndOfStream;
                    if tx_pb.send(msg).is_err() {
                        info!("Playback thread has already stopped.");
                    }
                    break;
                }
                Ok(AudioMessage::Pause) => {
                    trace!("AudioMessage::Pause received");
                    let msg = AudioMessage::Pause;
                    if tx_pb.send(msg).is_err() {
                        info!("Playback thread has already stopped.");
                        break;
                    }
                }
                Err(err) => {
                    error!("Message channel error: {}", err);
                    let msg = AudioMessage::EndOfStream;
                    if tx_pb.send(msg).is_err() {
                        info!("Playback thread has already stopped.");
                    }
                    break;
                }
            }
            if let Ok((diff, new_config)) = rx_pipeconf.try_recv() {
                trace!("Message received on config channel");
                match diff {
                    config::ConfigChange::Pipeline | config::ConfigChange::MixerParameters => {
                        debug!("Rebuilding pipeline.");
                        let new_pipeline =
                            filters::Pipeline::from_config(new_config, processing_params.clone());
                        pipeline = new_pipeline;
                    }
                    config::ConfigChange::FilterParameters {
                        filters,
                        mixers,
                        processors,
                    } => {
                        debug!(
                            "Updating parameters of filters: {:?}, mixers: {:?}.",
                            filters, mixers
                        );
                        pipeline.update_parameters(new_config, &filters, &mixers, &processors);
                    }
                    config::ConfigChange::Devices => {
                        let msg = AudioMessage::EndOfStream;
                        tx_pb.send(msg).unwrap();
                        break;
                    }
                    _ => {}
                };
            };
        }
        processing_params.set_processing_load(0.0);
        if let Some(h) = thread_handle {
            match demote_current_thread_from_real_time(h) {
                Ok(_) => {
                    debug!("Processing thread returned to normal priority.")
                }
                Err(_) => {
                    warn!("Could not bring the processing thread back to normal priority.")
                }
            };
        }
    })
}
