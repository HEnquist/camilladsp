// Traits for filters

pub trait Filter<T> {
    /// Filter a single point
    fn process_single(&mut self, input: T) -> T;

    // Filter a Vec
    fn process_multi(&mut self, input: Vec<T>) -> Vec<T>;
}