use crate::config;
use crate::filters::Filter;

use rand::rngs::SmallRng;
use rand::SeedableRng;
use rand_distr::{Distribution, Normal, Triangular, Uniform};

use crate::NewValue;
use crate::PrcFmt;
use crate::Res;

// lifetime `'a` guarantees that `ditherer` will live as long as this `Dither`.
pub struct Dither<'a> {
    pub name: String,
    pub scalefact: PrcFmt,
    buffer: Vec<PrcFmt>,
    filter: Vec<PrcFmt>,
    idx: usize,
    filterlen: usize,
    ditherer: Box<dyn Ditherer + 'a>,
}

pub type Coefficients = Vec<PrcFmt>;

impl<'a> Dither<'a> {
    pub fn new<D: Ditherer + 'a>(
        name: String,
        bits: usize,
        ditherer: D,
        filter: Option<Coefficients>,
    ) -> Self {
        let scalefact = PrcFmt::new(2.0).powi((bits - 1) as i32);
        let ditherer = Box::new(ditherer);

        // noise shaping
        let filter = filter.unwrap_or_default();
        let filterlen = filter.len();
        let buffer = vec![0.0; filterlen];
        let idx = 0;

        Self {
            name,
            scalefact,
            buffer,
            filter,
            idx,
            filterlen,
            ditherer,
        }
    }

    // Some filters borrowed from SOX: http://sox.sourceforge.net/SoX/NoiseShaping
    pub fn from_config(name: String, conf: config::DitherParameters) -> Self {
        match conf {
            config::DitherParameters::Simple { bits } => {
                let hp_tpdf = HighPassDitherer::default();
                Dither::new(name, bits, hp_tpdf, None)
            }
            config::DitherParameters::FweightedShort441 { bits } => {
                // F-weighted noise power: -10.47 dB
                // unweighted noise power:   6.64 dB
                let tpdf = TriangularDitherer::default();
                let filter = vec![1.623, -0.982, 0.109];
                Dither::new(name, bits, tpdf, Some(filter))
            }
            config::DitherParameters::Fweighted441 { bits } => {
                // F-weighted noise power: -16.80 dB
                // unweighted noise power:  18.40 dB
                let tpdf = TriangularDitherer::default();
                let filter = vec![
                    2.412, -3.370, 3.937, -4.174, 3.353, -2.205, 1.281, -0.569, 0.0847,
                ];
                Dither::new(name, bits, tpdf, Some(filter))
            }
            config::DitherParameters::FweightedLong441 { bits } => {
                // F-weighted noise power: -16.7 dB
                // unweighted noise power:  17.3 dB
                let tpdf = TriangularDitherer::default();
                let filter = vec![
                    2.391510, -3.284444, 3.679506, -3.635044, 2.524185, -1.146701, 0.115354,
                    0.513745, -0.749277, 0.512386, -0.749277, 0.512386, -0.188997, -0.043705,
                    0.149843, -0.151186, 0.076302, -0.012070, -0.021127, 0.025232, -0.016121,
                    0.004453, 0.000876, -0.001799, 0.000774, -0.000128,
                ];
                Dither::new(name, bits, tpdf, Some(filter))
            }
            config::DitherParameters::Gesemann441 { bits } => {
                let tpdf = TriangularDitherer::default();
                let filter = vec![
                    2.2061, -0.4706, -0.2534, -0.6214, 1.0587, 0.0676, -0.6054, -0.2738,
                ];
                Dither::new(name, bits, tpdf, Some(filter))
            }
            config::DitherParameters::Gesemann48 { bits } => {
                let tpdf = TriangularDitherer::default();
                let filter = vec![
                    2.2374, -0.7339, -0.1251, -0.6033, 0.903, 0.0116, -0.5853, -0.2571,
                ];
                Dither::new(name, bits, tpdf, Some(filter))
            }
            config::DitherParameters::Lipshitz441 { bits } => {
                // E-weighted noise power: -14.34 dB
                // unweighted noise power:  12.19 dB
                let tpdf = TriangularDitherer::default();
                let filter = vec![2.033, -2.165, 1.959, -1.590, 0.6149];
                Dither::new(name, bits, tpdf, Some(filter))
            }
            config::DitherParameters::LipshitzLong441 { bits } => {
                // E-weighted noise power: -18.32 dB
                // unweighted noise power:  23.10 dB
                let tpdf = TriangularDitherer::default();
                let filter = vec![
                    2.847, -4.685, 6.214, -7.184, 6.639, -5.032, 3.263, -1.632, 0.4191,
                ];
                Dither::new(name, bits, tpdf, Some(filter))
            }
            config::DitherParameters::Shibata441 { bits } => {
                let tpdf = TriangularDitherer::default();
                let filter = vec![
                    2.677_319_765_090_942_4,
                    -4.830_892_562_866_211,
                    6.570_110_321_044_922,
                    -7.457_201_480_865_478_5,
                    6.726_327_419_281_006,
                    -4.848_165_035_247_803,
                    2.041_208_982_467_651_4,
                    0.700_635_910_034_179_7,
                    -2.953_756_570_816_04,
                    4.080_038_547_515_869,
                    -4.184_521_675_109_863,
                    3.331_181_287_765_503,
                    -2.117_992_639_541_626,
                    0.879_302_978_515_625,
                    -0.031_759_146_600_961_685,
                    -0.423_827_886_581_420_9,
                    0.478_821_039_199_829_1,
                    -0.354_908_138_513_565_06,
                    0.174_968_391_656_875_6,
                    -0.060_908_168_554_306_03,
                ];
                Dither::new(name, bits, tpdf, Some(filter))
            }
            config::DitherParameters::Shibata48 { bits } => {
                let tpdf = TriangularDitherer::default();
                let filter = vec![
                    2.872_072_935_104_37,
                    -5.041_323_184_967_041,
                    6.244_299_411_773_682,
                    -5.848_398_685_455_322,
                    3.706_754_207_611_084,
                    -1.049_511_909_484_863_3,
                    -1.183_023_691_177_368_2,
                    2.112_679_243_087_768_6,
                    -1.909_453_153_610_229_5,
                    0.999_130_845_069_885_3,
                    -0.170_908_063_650_131_23,
                    -0.326_156_020_164_489_75,
                    0.391_276_448_965_072_63,
                    -0.268_764_615_058_898_9,
                    0.097_676_105_797_290_8,
                    -0.023_473_845_794_796_944,
                ];
                Dither::new(name, bits, tpdf, Some(filter))
            }
            config::DitherParameters::ShibataHigh441 { bits } => {
                let tpdf = TriangularDitherer::default();
                let filter = vec![
                    3.025_918_960_571_289,
                    -6.026_871_681_213_379,
                    9.195_003_509_521_484,
                    -11.824_929_237_365_723,
                    12.767_142_295_837_402,
                    -11.917_946_815_490_723,
                    9.173_916_816_711_426,
                    -5.371_232_032_775_879,
                    1.139_362_454_414_367_7,
                    2.448_477_983_474_731_4,
                    -4.971_983_909_606_934,
                    6.039_200_305_938_721,
                    -5.935_952_186_584_473,
                    4.903_278_350_830_078,
                    -3.552_744_388_580_322_3,
                    2.190_969_705_581_665,
                    -1.167_238_950_729_370_1,
                    0.490_391_433_238_983_15,
                    -0.165_197_908_878_326_42,
                    0.023_217_858_746_647_835,
                ];
                Dither::new(name, bits, tpdf, Some(filter))
            }
            config::DitherParameters::ShibataLow441 { bits } => {
                let tpdf = TriangularDitherer::default();
                let filter = vec![
                    2.083_391_666_412_353_5,
                    -3.041_845_083_236_694_3,
                    3.204_789_876_937_866,
                    -2.757_192_611_694_336,
                    1.497_863_054_275_512_7,
                    -0.342_759_460_210_800_17,
                    -0.717_337_489_128_112_9,
                    1.073_705_792_427_063,
                    -1.022_581_577_301_025_4,
                    0.566_499_948_501_586_9,
                    -0.209_686_920_046_806_34,
                    -0.065_378_531_813_621_52,
                    0.103_224_381_804_466_25,
                    -0.067_442_022_264_003_75,
                    -0.004_951_973_445_713_52,
                ];
                Dither::new(name, bits, tpdf, Some(filter))
            }
            config::DitherParameters::ShibataLow48 { bits } => {
                let tpdf = TriangularDitherer::default();
                let filter = vec![
                    2.392_577_409_744_262_7,
                    -3.435_029_745_101_928_7,
                    3.185_370_922_088_623,
                    -1.811_727_166_175_842_3,
                    -0.201_247_707_009_315_5,
                    1.475_990_772_247_314_5,
                    -1.721_090_435_981_750_5,
                    0.977_467_000_484_466_6,
                    -0.137_901_380_658_149_72,
                    -0.381_859_034_299_850_46,
                    0.274_212_419_986_724_85,
                    0.066_584_214_568_138_12,
                    -0.352_233_022_451_400_76,
                    0.376_723_438_501_358_03,
                    -0.239_642_769_098_281_86,
                    0.068_674_825_131_893_16,
                ];
                Dither::new(name, bits, tpdf, Some(filter))
            }
            config::DitherParameters::Gaussian { bits, amplitude } => {
                let gpdf = <GaussianDitherer as Ditherer>::new(amplitude);
                Dither::new(name, bits, gpdf, None)
            }
            config::DitherParameters::Triangular { bits, amplitude } => {
                let tpdf = <TriangularDitherer as Ditherer>::new(amplitude);
                Dither::new(name, bits, tpdf, None)
            }
            config::DitherParameters::Uniform { bits, amplitude } => {
                let rpdf = <UniformDitherer as Ditherer>::new(amplitude);
                Dither::new(name, bits, rpdf, None)
            }
            config::DitherParameters::None { bits } => {
                let none = <NoopDitherer as Ditherer>::new(0.0);
                Dither::new(name, bits, none, None)
            }
        }
    }
}

impl<'a> Filter for Dither<'a> {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn process_waveform(&mut self, waveform: &mut [PrcFmt]) -> Res<()> {
        if self.filterlen > 0 {
            for item in waveform.iter_mut() {
                let scaled = *item * self.scalefact;
                let mut filt_buf = 0.0;
                for (n, coeff) in self.filter.iter().enumerate() {
                    filt_buf += coeff * self.buffer[(n + self.idx) % self.filterlen];
                }
                if self.idx > 0 {
                    self.idx -= 1;
                } else {
                    self.idx = self.filterlen - 1;
                }
                let scaled_plus_err = scaled + filt_buf;
                let result = scaled_plus_err + self.ditherer.sample();

                let result_r = result.round();
                self.buffer[self.idx] = scaled_plus_err - result_r;
                *item = result_r / self.scalefact;
            }
        } else {
            for item in waveform.iter_mut() {
                let scaled = *item * self.scalefact + self.ditherer.sample();
                *item = scaled.round() / self.scalefact;
            }
        }

        Ok(())
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Dither {
            parameters: conf, ..
        } = conf
        {
            let name = self.name.clone();
            *self = Dither::from_config(name, conf);
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

/// Validate a Dither config.
pub fn validate_config(conf: &config::DitherParameters) -> Res<()> {
    let bits = match conf {
        config::DitherParameters::Simple { bits }
        | config::DitherParameters::FweightedShort441 { bits }
        | config::DitherParameters::Fweighted441 { bits }
        | config::DitherParameters::FweightedLong441 { bits }
        | config::DitherParameters::Gesemann441 { bits }
        | config::DitherParameters::Gesemann48 { bits }
        | config::DitherParameters::Lipshitz441 { bits }
        | config::DitherParameters::LipshitzLong441 { bits }
        | config::DitherParameters::Shibata441 { bits }
        | config::DitherParameters::Shibata48 { bits }
        | config::DitherParameters::ShibataHigh441 { bits }
        | config::DitherParameters::ShibataLow441 { bits }
        | config::DitherParameters::ShibataLow48 { bits }
        | config::DitherParameters::Gaussian { bits, .. }
        | config::DitherParameters::Triangular { bits, .. }
        | config::DitherParameters::Uniform { bits, .. }
        | config::DitherParameters::None { bits } => bits,
    };
    if *bits <= 1 {
        return Err(config::ConfigError::new("Dither bit depth must be at least 2").into());
    }

    if let config::DitherParameters::Gaussian { amplitude, .. }
    | config::DitherParameters::Triangular { amplitude, .. }
    | config::DitherParameters::Uniform { amplitude, .. } = conf
    {
        if *amplitude < 0.0 {
            return Err(config::ConfigError::new("Dither amplitude cannot be negative").into());
        }
        if *amplitude > 100.0 {
            return Err(config::ConfigError::new("Dither amplitude must be less than 100").into());
        }
    }

    Ok(())
}

// Adopted from librespot, with permission:
// Ditherer, TriangularDitherer, HighPassDitherer.
pub trait Ditherer {
    // `amplitude` in bits
    fn new(amplitude: PrcFmt) -> Self
    where
        Self: Sized;

    fn sample(&mut self) -> PrcFmt;
}

fn create_rng() -> SmallRng {
    SmallRng::from_entropy()
}

// Simple rectangular-pdf (RPDF; uniform) dither.
// Suboptimal to TPDF, because the residual noise level varies audibly.
#[derive(Clone, Debug)]
pub struct UniformDitherer {
    cached_rng: SmallRng,
    distribution: Uniform<PrcFmt>,
}

impl Ditherer for UniformDitherer {
    fn new(amplitude: PrcFmt) -> Self {
        let amplitude = amplitude / 2.0;
        Self {
            cached_rng: create_rng(),
            distribution: Uniform::new_inclusive(-amplitude, amplitude),
        }
    }

    fn sample(&mut self) -> PrcFmt {
        self.distribution.sample(&mut self.cached_rng)
    }
}

impl Default for UniformDitherer {
    fn default() -> Self {
        <Self as Ditherer>::new(1.0)
    }
}

// Spectrally-white triangular-pdf (TPDF) dither.
// The high-pass ditherer should be preferred for most audio applications.
#[derive(Clone, Debug)]
pub struct TriangularDitherer {
    cached_rng: SmallRng,
    distribution: Triangular<PrcFmt>,
}

impl Ditherer for TriangularDitherer {
    fn new(amplitude: PrcFmt) -> Self {
        let amplitude = amplitude / 2.0;
        Self {
            cached_rng: create_rng(),
            distribution: Triangular::new(-amplitude, amplitude, 0.0).unwrap(),
        }
    }

    fn sample(&mut self) -> PrcFmt {
        self.distribution.sample(&mut self.cached_rng)
    }
}

impl Default for TriangularDitherer {
    fn default() -> Self {
        // 2 LSB peak-to-peak needed to linearize the response
        <Self as Ditherer>::new(2.0)
    }
}

// A very simple discrete-time noise generator capable of producing dither with
// a high-pass spectrum, and all the beneficial effects of the TPDF dither.
// Preferable in audio since it is less audible than spectrally-white TPDF
// dither. Furthermore, it is more computationally efficient since it requires
// the calculation of only one new RPDF random number per sampling period.
#[derive(Clone, Debug)]
pub struct HighPassDitherer {
    ditherer: UniformDitherer,
    previous_sample: PrcFmt,
}

impl Ditherer for HighPassDitherer {
    fn new(amplitude: PrcFmt) -> Self {
        Self {
            // 2x RDPF sample (current - previous) makes 1x TDPF
            ditherer: <UniformDitherer as Ditherer>::new(amplitude / 2.0),
            previous_sample: 0.0,
        }
    }

    fn sample(&mut self) -> PrcFmt {
        let new_sample = self.ditherer.sample();
        let high_passed_sample = new_sample - self.previous_sample;
        self.previous_sample = new_sample;
        high_passed_sample
    }
}

impl Default for HighPassDitherer {
    fn default() -> Self {
        // 1 LSB +/- 1 LSB (previous) = 2 LSB
        <Self as Ditherer>::new(2.0)
    }
}

// Spectrally-white Gaussian-pdf (GPDF) dither.
// TPDF is better objectively, but some may find this sounds more "analog".
#[derive(Clone, Debug)]
pub struct GaussianDitherer {
    cached_rng: SmallRng,
    distribution: Normal<PrcFmt>,
}

impl Ditherer for GaussianDitherer {
    fn new(amplitude: PrcFmt) -> Self {
        Self {
            cached_rng: create_rng(),
            distribution: Normal::new(0.0, amplitude).unwrap(),
        }
    }

    fn sample(&mut self) -> PrcFmt {
        self.distribution.sample(&mut self.cached_rng)
    }
}

impl Default for GaussianDitherer {
    fn default() -> Self {
        // 1/2 LSB RMS needed to linearize the response
        <Self as Ditherer>::new(0.5)
    }
}

// No-op ditherer.
// Cheaper than checking for an `Option` in `process_waveform()`.
#[derive(Clone, Debug)]
pub struct NoopDitherer;

impl Ditherer for NoopDitherer {
    fn new(_amplitude: PrcFmt) -> Self {
        Self {}
    }

    fn sample(&mut self) -> PrcFmt {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use crate::config::DitherParameters;
    use crate::dither::{Dither, Ditherer, NoopDitherer, UniformDitherer};
    use crate::filters::Filter;
    use crate::PrcFmt;

    fn is_close(left: PrcFmt, right: PrcFmt, maxdiff: PrcFmt) -> bool {
        println!("{} - {}", left, right);
        (left - right).abs() < maxdiff
    }

    fn compare_waveforms(left: Vec<PrcFmt>, right: Vec<PrcFmt>, maxdiff: PrcFmt) -> bool {
        for (val_l, val_r) in left.iter().zip(right.iter()) {
            if !is_close(*val_l, *val_r, maxdiff) {
                return false;
            }
        }
        true
    }

    #[test]
    fn test_quantize() {
        let mut waveform = vec![-1.0, -0.5, -1.0 / 3.0, 0.0, 1.0 / 3.0, 0.5, 1.0];
        let waveform2 = waveform.clone();
        let mut dith = Dither::new(
            "test".to_string(),
            8,
            <NoopDitherer as Ditherer>::new(0.0),
            None,
        );
        dith.process_waveform(&mut waveform).unwrap();
        assert!(compare_waveforms(
            waveform.clone(),
            waveform2.clone(),
            1.0 / 128.0
        ));
        assert!(is_close(
            (128.0 * waveform[2]).round(),
            128.0 * waveform[2],
            1e-9
        ));
    }

    #[test]
    fn test_uniform() {
        let mut waveform = vec![-1.0, -0.5, -1.0 / 3.0, 0.0, 1.0 / 3.0, 0.5, 1.0];
        let waveform2 = waveform.clone();
        let mut dith = Dither::new("test".to_string(), 8, UniformDitherer::default(), None);
        dith.process_waveform(&mut waveform).unwrap();
        assert!(compare_waveforms(
            waveform.clone(),
            waveform2.clone(),
            1.0 / 64.0
        ));
        assert!(is_close(
            (128.0 * waveform[2]).round(),
            128.0 * waveform[2],
            1e-9
        ));
    }

    #[test]
    fn test_simple() {
        let mut waveform = vec![-1.0, -0.5, -1.0 / 3.0, 0.0, 1.0 / 3.0, 0.5, 1.0];
        let waveform2 = waveform.clone();
        let conf = DitherParameters::Simple { bits: 8 };
        let mut dith = Dither::from_config("test".to_string(), conf);
        dith.process_waveform(&mut waveform).unwrap();
        assert!(compare_waveforms(
            waveform.clone(),
            waveform2.clone(),
            1.0 / 32.0
        ));
        assert!(is_close(
            (128.0 * waveform[2]).round(),
            128.0 * waveform[2],
            1e-9
        ));
    }

    #[test]
    fn test_lip() {
        let mut waveform = vec![-1.0, -0.5, -1.0 / 3.0, 0.0, 1.0 / 3.0, 0.5, 1.0];
        let waveform2 = waveform.clone();
        let conf = DitherParameters::Lipshitz441 { bits: 8 };
        let mut dith = Dither::from_config("test".to_string(), conf);
        dith.process_waveform(&mut waveform).unwrap();
        assert!(compare_waveforms(
            waveform.clone(),
            waveform2.clone(),
            1.0 / 16.0
        ));
        assert!(is_close(
            (128.0 * waveform[2]).round(),
            128.0 * waveform[2],
            1e-9
        ));
    }
}
