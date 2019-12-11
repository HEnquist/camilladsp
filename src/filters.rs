// Traits for filters
// Sample format
type SmpFmt = i16;
type PrcFmt = f64;

pub trait Filter {
    // Filter a Vec
    fn process_waveform(&mut self, input: Vec<PrcFmt>) -> Vec<PrcFmt>;
}