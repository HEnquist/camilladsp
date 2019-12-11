use std::io::BufReader;
use std::io::BufRead;
use std::fs::File;
use std::path::Path;
use std::{iter, error};

pub type Res<T> = Result<T, Box<dyn error::Error>>;

// Traits etc for filters
// Sample format
type SmpFmt = i16;
type PrcFmt = f64;

pub trait Filter {
    // Filter a Vec
    fn process_waveform(&mut self, input: Vec<PrcFmt>) -> Vec<PrcFmt>;
}

pub fn read_coeff_file(filename: &str) -> Res<Vec<PrcFmt>> {
    let mut coefficients = Vec::<PrcFmt>::new();
    let f = File::open(filename).unwrap();
    let mut file = BufReader::new(&f);
    for line in file.lines() {
        let l = line.unwrap();
        coefficients.push(l.parse().unwrap());
        
    }
    println!("{:?}", coefficients); 
    Ok(coefficients)
}