// TEMPORARY: sizing experiment for interleaved multi-channel biquads.
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use camillalib::CamillaFloat;

const CHUNK: usize = 1024;

#[inline(always)]
fn mul_add(a: CamillaFloat, b: CamillaFloat, c: CamillaFloat) -> CamillaFloat {
    if cfg!(any(
        all(target_arch = "aarch64", target_feature = "neon"),
        target_feature = "fma",
    )) {
        a.mul_add(b, c)
    } else {
        a * b + c
    }
}

struct Bank<const N: usize> {
    s1: [CamillaFloat; N],
    s2: [CamillaFloat; N],
    b0: [CamillaFloat; N],
    b1: [CamillaFloat; N],
    b2: [CamillaFloat; N],
    a1: [CamillaFloat; N],
    a2: [CamillaFloat; N],
}

impl<const N: usize> Bank<N> {
    fn new() -> Self {
        Bank {
            s1: [0.0; N],
            s2: [0.0; N],
            b0: [0.21476322779271284; N],
            b1: [0.4295264555854257; N],
            b2: [0.21476322779271284; N],
            a1: [-0.1462978543780541; N],
            a2: [0.005350765548905586; N],
        }
    }

    /// Interleave N independent biquad recurrences over N planar channels.
    #[inline(never)]
    fn process(&mut self, chans: &mut [Vec<CamillaFloat>]) {
        let len = chans[0].len();
        for i in 0..len {
            for c in 0..N {
                let x = chans[c][i];
                let out = mul_add(self.b0[c], x, self.s1[c]);
                self.s1[c] = mul_add(-self.a1[c], out, mul_add(self.b1[c], x, self.s2[c]));
                self.s2[c] = mul_add(-self.a2[c], out, self.b2[c] * x);
                chans[c][i] = out;
            }
        }
    }
}

fn run<const N: usize>(c: &mut Criterion, group: &mut Option<()>) {
    let _ = group;
    let mut bank = Bank::<N>::new();
    let mut chans: Vec<Vec<CamillaFloat>> = (0..N).map(|_| vec![0.0; CHUNK]).collect();
    let mut g = c.benchmark_group("interleave");
    g.throughput(criterion::Throughput::Elements((N * CHUNK) as u64));
    g.bench_with_input(BenchmarkId::from_parameter(N), &N, |b, _| {
        b.iter(|| bank.process(&mut chans))
    });
    g.finish();
}

fn bench(c: &mut Criterion) {
    run::<1>(c, &mut None);
    run::<2>(c, &mut None);
    run::<3>(c, &mut None);
    run::<4>(c, &mut None);
    run::<6>(c, &mut None);
    run::<8>(c, &mut None);
    run::<12>(c, &mut None);
}

criterion_group!(benches, bench);
criterion_main!(benches);
