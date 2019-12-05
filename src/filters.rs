// Traits for filters
// Sample format
type SampleFormat = i16;
type ProcessingFormat = f64;

pub trait Filter {
    /// Filter a single point
    fn process_single(&mut self, input: ProcessingFormat) -> ProcessingFormat;

    // Filter a Vec
    fn process_multi(&mut self, input: Vec<ProcessingFormat>) -> Vec<ProcessingFormat>;
}