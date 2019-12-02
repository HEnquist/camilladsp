extern crate alsa;
use std::{iter, error};
use alsa::{Direction, ValueOr};
use alsa::pcm::{PCM, HwParams, Format, Access, State};
use alsa::direct::pcm::MmapPlayback;
use std::{thread, time};
use std::sync::mpsc;

type Res<T> = Result<T, Box<dyn error::Error>>;

mod filters;
use filters::*;
mod biquad;
use biquad::*;

mod audiodevice;
mod alsadevice;
use audiodevice::*;
use alsadevice::*;

//pub use crate::filters::*;
//pub use crate::biquad::*;



// Sample format
type SF = i16;
type PF = f64;


enum Message {
    Quit,
    Audio(AudioChunk),
}



fn run() -> Res<()> {
    let mut playback_dev = AlsaPlaybackDevice::<i16>::open("hw:PCH".to_string(), 44100, 4096, 2)?;
    let mut capture_dev = AlsaCaptureDevice::<i16>::open("hw:Loopback,0,0".to_string(), 44100, 4096, 2)?;
    //let (playback_dev, play_rate) = open_audio_dev_play("hw:PCH".to_string(), 44100, 1024)?;
    //let (capture_dev, capt_rate) = open_audio_dev_capt("hw:PCH".to_string(), 44100, 1024)?;

    
    let (tx_pb, rx_pb) = mpsc::channel();
    let (tx_cap, rx_cap) = mpsc::channel();

    //let mut mmap = playback_dev.direct_mmap_playback::<SF>()?;

    thread::spawn(move || {
        let coeffs_32 = Coefficients::<f32>::new(-1.79907162, 0.81748736, 0.00460394, 0.00920787, 0.00460394);
        let mut filter_l_32 = BiquadDF2T::<f32>::new(coeffs_32);
        let mut filter_r_32 = BiquadDF2T::<f32>::new(coeffs_32);
        let coeffs_64 = Coefficients::<f64>::new(-1.79907162, 0.81748736, 0.00460394, 0.00920787, 0.00460394);
        let mut filter_l_64 = BiquadDF2T::<f64>::new(coeffs_64);
        let mut filter_r_64 = BiquadDF2T::<f64>::new(coeffs_64);
        loop {
            match rx_cap.recv() {
                Ok(Message::Audio(chunk)) => {
                    //let mut buf = vec![0f64; 1024];
                    //for (i, a) in buf.iter_mut().enumerate() {
                    //    *a = (i as f64 * 2.0 * ::std::f64::consts::PI / 128.0).sin();
                    //}
                    let waveforms = match chunk.waveforms {
                        Waveforms::Float32(mut wfs) => {
                            let mut filtered_wfs = Vec::new();
                            //for wave in wfs.iter() {
                            let filtered_l = filter_l_32.process_multi(wfs[0].clone());
                            filtered_wfs.push(filtered_l);
                            let filtered_r = filter_r_32.process_multi(wfs[1].clone());
                            filtered_wfs.push(filtered_r);
                            Waveforms::Float32(filtered_wfs)
                        },
                        Waveforms::Float64(mut wfs) => {
                            let mut filtered_wfs = Vec::new();
                            //for wave in wfs.iter() {
                            let filtered_l = filter_l_64.process_multi(wfs.pop().unwrap());
                            //let filtered_l = wfs[0].clone();
                            filtered_wfs.push(filtered_l);
                            let filtered_r = filter_r_64.process_multi(wfs.pop().unwrap());
                            //let filtered_r = wfs[1].clone();
                            filtered_wfs.push(filtered_r);
                            Waveforms::Float64(filtered_wfs)
                        },
                    };

                    let chunk = AudioChunk{
                        frames: 4096,
                        channels: 2,
                        waveforms: waveforms,
                        //waveforms: Waveforms::Float64(vec![buf.clone(), buf]),
                    };
                    let msg = Message::Audio(chunk);
                    tx_pb.send(msg).unwrap();
                }
                _ => {}
            }
        }
    });

    thread::spawn(move || {
        let delay = time::Duration::from_millis(8*1000*1024/44100);
        thread::sleep(delay);
        let mut m = 0;
        loop {
            match rx_pb.recv() {
                Ok(Message::Audio(chunk)) => {
                    playback_dev.put_chunk(chunk).unwrap();
                    let frames = playback_dev.play().unwrap();
                    println!("PB Chunk {}, wrote {:?} frames", m, frames);
                    m += 1;
                }
                _ => {}
            }
        }
    });

    thread::spawn(move || {
        let mut m = 0;
        loop {
            let frames = capture_dev.capture().unwrap();
            let chunk = capture_dev.fetch_chunk(Datatype::Float32).unwrap();
            let msg = Message::Audio(chunk);
            tx_cap.send(msg).unwrap();
            println!("Capture chunk {}", m);
            m += 1;
        }
    });

    let delay = time::Duration::from_millis(100);
    

    loop {
        thread::sleep(delay);
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() { println!("Error ({}) {}", e.description(), e); }
}
