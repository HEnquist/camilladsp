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
    let mut playback_dev = AlsaPlaybackDevice::<i16>::open("hw:PCH".to_string(), 44100, 1024, 2)?;
    let mut capture_dev = AlsaCaptureDevice::<i16>::open("hw:PCH".to_string(), 44100, 1024, 2)?;
    //let (playback_dev, play_rate) = open_audio_dev_play("hw:PCH".to_string(), 44100, 1024)?;
    //let (capture_dev, capt_rate) = open_audio_dev_capt("hw:PCH".to_string(), 44100, 1024)?;

    
    let (tx_pb, rx_pb) = mpsc::channel();
    let (tx_cap, rx_cap) = mpsc::channel();

    //let mut mmap = playback_dev.direct_mmap_playback::<SF>()?;

    thread::spawn(move || {
        let coeffs = Coefficients::<f64>::new(-1.97984856, 0.98004953, 5.02413473e-5, 1.00482695e-4, 5.02413473e-5);
        let mut filter = BiquadDF2T::<f64>::new(coeffs);
        loop {
            match rx_cap.recv() {
                Ok(Message::Audio(chunk)) => {
                    let mut buf = vec![0f64; 1024];
                    for (i, a) in buf.iter_mut().enumerate() {
                        *a = (i as f64 * 2.0 * ::std::f64::consts::PI / 128.0).sin();
                    }
                    buf = filter.process_multi(buf);

                    let chunk = AudioChunk{
                        frames: 1024,
                        channels: 2,
                        waveforms: Waveforms::Float64(vec![buf.clone(), buf]),
                    };
                    let msg = Message::Audio(chunk);
                    tx_pb.send(msg).unwrap();
                }
                _ => {}
            }
        }
    });

    thread::spawn(move || {
        let delay = time::Duration::from_millis(2*1000*1024/44100);
        thread::sleep(delay);
        //let mut io_play = playback_dev.io_i16().unwrap();
        let mut m = 0;
        loop {
            match rx_pb.recv() {
                Ok(Message::Audio(chunk)) => {
                    //let buf = chunk.to_interleaved();
                    playback_dev.put_chunk(chunk).unwrap();
                    let frames = playback_dev.play().unwrap();
                    //let frames = play_from_buffer(&playback_dev, &mut io_play, buf);
                    println!("PB Chunk {}, wrote {:?} frames", m, frames);
                    m += 1;
                }
                _ => {}
            }
        }
    });

    thread::spawn(move || {
        //let mut io_capt = capture_dev.io_i16().unwrap();
        let mut m = 0;
        loop {
            //let buf = capture_to_buffer(&capture_dev, &mut io_capt, 2, 1024).unwrap();
            //let chunk = AudioChunk::from_interleaved(buf, 2);
            let frames = capture_dev.capture().unwrap();
            let chunk = capture_dev.fetch_chunk(Datatype::Float64).unwrap();
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
