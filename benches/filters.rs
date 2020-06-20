extern crate criterion;
use criterion::{criterion_group, criterion_main, Bencher, BenchmarkId, Criterion};
extern crate camillalib;

use camillalib::fftconv::FFTConv;
use camillalib::PrcFmt;
use camillalib::filters::Filter;


/// Times just the FFT execution (not allocation and pre-calculation)
/// for a given length
fn run_conv(b: &mut Bencher, len: usize, chunksize: usize) {

    let filter = vec![0.0 as PrcFmt; len];
    let mut conv = FFTConv::new("test".to_string(), chunksize, &filter);
    let mut waveform = vec![0.0 as PrcFmt; chunksize];

    //let mut spectrum = signal.clone();
    b.iter(|| conv.process_waveform(&mut waveform));
}



fn bench_conv(c: &mut Criterion) {
    let mut group = c.benchmark_group("Conv");
    let chunksize = 1024;
    for filterlen in [chunksize, 4*chunksize, 16*chunksize].iter() {
        group.bench_with_input(BenchmarkId::new("FFTConv", filterlen), filterlen, |b, filterlen| run_conv(b, *filterlen, chunksize));
    }
    group.finish();
}



criterion_group!(benches, bench_conv);

criterion_main!(benches);
