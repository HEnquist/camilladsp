use circular_queue::CircularQueue;
use rand::{rngs::SmallRng, SeedableRng};
use rand_distr::{Distribution, Triangular, Uniform};

use crate::{config, filters::Filter, NewValue, PrcFmt, Res};

// lifetime `'a` to guarantee that `ditherer` and `shaper`
// will live as long as this `Dither`.
pub struct Dither<'a> {
    pub name: String,
    pub scalefact: PrcFmt,
    // have to `Box` because `dyn Ditherer` is not `Sized`.
    ditherer: Box<dyn Ditherer + 'a>,
    shaper: Option<NoiseShaper<'a>>,
}

#[derive(Clone, Debug)]
pub struct NoiseShaper<'a> {
    // optimization: lifetime allows taking coefficients
    // from an array instead of allocating a `Vec`.
    filter: &'a [PrcFmt],
    buffer: CircularQueue<PrcFmt>,
}

impl<'a> NoiseShaper<'a> {
    pub fn new(filter: &'a [PrcFmt]) -> Self {
        let buffer = CircularQueue::with_capacity(filter.len());
        Self { filter, buffer }
    }

    // Source: Wannamaker, R.A. (1992). Psychoacoustically Optimal Noise Shaping.
    // University of Waterloo.
    pub fn fweighted_441() -> Self {
        Self::new(&[
            2.412, -3.370, 3.937, -4.174, 3.353, -2.205, 1.281, -0.569, 0.0847,
        ])
    }

    pub fn fweighted_long_441() -> Self {
        Self::new(&[
            2.391510, -3.284444, 3.679506, -3.635044, 2.524185, -1.146701, 0.115354, 0.513745,
            -0.749277, 0.512386, -0.749277, 0.512386, -0.188997, -0.043705, 0.149843, -0.151186,
            0.076302, -0.012070, -0.021127, 0.025232, -0.016121, 0.004453, 0.000876, -0.001799,
            0.000774, -0.000128,
        ])
    }

    pub fn fweighted_short_441() -> Self {
        Self::new(&[1.623, -0.982, 0.109])
    }

    // Source: Sebastian Gesemann, archived at: https://web.archive.org/web/20100710070155/http://www.hydrogenaudio.org/forums/index.php?showtopic=47980&pid=551558&st=0
    pub fn gesemann_441() -> Self {
        Self::new(&[
            2.2061, -0.4706, -0.2534, -0.6214, 1.0587, 0.0676, -0.6054, -0.2738,
        ])
    }

    pub fn gesemann_48() -> Self {
        Self::new(&[
            2.2374, -0.7339, -0.1251, -0.6033, 0.903, 0.0116, -0.5853, -0.2571,
        ])
    }

    // Source: Lipshitz, S.P., Vanderkooy, J. & Wannamaker, R.A. (1991).
    // Minimally Audible Noise Shaping. University of Waterloo.
    pub fn lipshitz_441() -> Self {
        Self::new(&[2.033, -2.165, 1.959, -1.590, 0.6149])
    }

    pub fn lipshitz_long_441() -> Self {
        Self::new(&[
            2.847, -4.685, 6.214, -7.184, 6.639, -5.032, 3.263, -1.632, 0.4191,
        ])
    }

    // Shibata filters by Naoki Shibata: https://shibatch.sourceforge.net
    // Derived from SSRC 1.32: https://github.com/shibatch/SSRC/blob/master/shapercoefs.h
    pub fn shibata_441() -> Self {
        Self::new(&[
            1.356_863_856_315_612_8,
            -1.225_293_517_112_732,
            0.623_555_064_201_355,
            -0.225_620_940_327_644_35,
            -0.235_579_758_882_522_58,
            0.135_363_623_499_870_3,
            -0.091_538_146_138_191_22,
            -0.056_445_639_580_488_205,
            3.961_442_416_766_658_4e-5,
            -0.023_561_919_108_033_18,
            -0.010_756_319_388_747_215,
            -0.000_319_491_315_167_397_26,
            0.001_433_762_023_225_426_7,
            -0.008_455_123_752_355_576,
            -0.000_213_181_803_701_445_46,
            7.617_592_200_404_033e-5,
            0.001_010_233_070_701_360_7,
            4.503_027_594_182_64e-5,
            0.001_343_382_173_217_833,
            0.001_393_724_232_912_063_6,
            0.000_433_067_005_360_499,
            0.000_469_497_870_653_867_7,
            0.000_147_758_415_550_924_84,
            -4.106_017_513_549_886_6e-5,
        ])
    }

    pub fn shibata_high_441() -> Self {
        Self::new(&[
            2.826_326_608_657_837,
            -5.353_435_993_194_58,
            7.804_205_894_470_215,
            -9.679_368_972_778_32,
            10.157135009765625,
            -9.439_995_765_686_035,
            7.614_612_579_345_703,
            -5.424_517_631_530_762,
            3.247_828_245_162_964,
            -1.630_185_246_467_590_3,
            0.585_380_196_571_350_1,
            -0.117_100_022_733_211_52,
            -0.033_543_668_687_343_6,
            0.008_884_146_809_577_942,
            0.017_314_357_683_062_553,
            -0.033_262_729_644_775_39,
            0.018_168_220_296_502_113,
            -0.006_801_502_779_126_167,
            -0.000_969_119_486_398_994_9,
            0.000_964_893_435_593_694_4,
        ])
    }

    pub fn shibata_low_441() -> Self {
        Self::new(&[
            0.595_437_824_726_104_7,
            -0.002_507_873_112_335_801,
            -0.185_180_589_556_694_03,
            -0.001_037_429_319_694_638_3,
            -0.103_663_429_617_881_77,
            -0.053_248_628_973_960_876,
            -8.403_004_903_811_961e-5,
            -3.856_993_302_520_095e-8,
            -0.026_413_010_433_316_23,
            -0.000_684_383_965_563_029,
            3.158_050_503_770_937_2e-6,
            0.031_739_629_805_088_04,
        ])
    }

    pub fn shibata_48() -> Self {
        Self::new(&[
            1.491_957_783_699_035_6,
            -1.308_917_880_058_288_6,
            0.540_516_316_890_716_6,
            -0.000_361_137_499_567_121_27,
            -0.363_031_953_573_226_93,
            0.109_111_279_249_191_28,
            0.007_310_638_204_216_957,
            -0.115_459_144_115_448,
            0.003_772_285_534_068_942,
            -0.012_545_258_738_100_529,
            -0.029_272_487_387_061_12,
            -0.005_002_200_137_823_82,
            -0.000_202_188_515_686_430_04,
            -0.004_905_734_676_867_723_5,
            -0.005_127_976_182_848_215,
            -0.002_505_671_000_108_123,
        ])
    }

    pub fn shibata_high_48() -> Self {
        Self::new(&[
            3.260_151_624_679_565_4,
            -6.557_569_503_784_18,
            9.748_664_855_957_031,
            -11.713_088_989_257_813,
            11.504_628_181_457_52,
            -9.485_962_867_736_816,
            6.404_273_033_142_09,
            -3.477_282_047_271_728_5,
            1.332_738_280_296_325_7,
            -0.264_645_755_290_985_1,
            -0.081_823_304_295_539_86,
            0.044_643_409_550_189_97,
            0.021_642_472_594_976_425,
            -0.042_832_121_253_013_61,
            0.003_383_262_082_934_379_6,
            0.016_050_558_537_244_797,
            -0.019_443_769_007_921_22,
            0.002_014_045_603_573_322_3,
            0.005_101_846_531_033_516,
            -0.004_944_144_282_490_015,
            -0.001_399_693_894_200_027,
            0.003_581_011_900_678_277,
            -0.002_209_919_737_651_944,
            -0.000_101_200_050_266_925_25,
            0.000_771_208_666_265_010_8,
            -4.772_754_982_695_915e-5,
            -0.000_470_578_757_813_200_35,
            0.000_535_220_140_591_263_8,
        ])
    }

    pub fn shibata_low_48() -> Self {
        Self::new(&[
            0.648_154_377_937_316_9,
            -0.000_132_923_290_948_383_5,
            -0.152_844_399_213_790_9,
            -0.024_795_081_466_436_386,
            -0.028_879_294_171_929_36,
            -0.097_741_305_828_094_48,
            3.723_334_521_055_221_6e-5,
            3.036_181_624_338_496_5e-6,
            -2.685_151_775_949_634_6e-5,
            -0.015_118_855_983_018_875,
            -0.000_119_081_560_114_864_26,
            4.020_391_770_609_422e-6,
            0.032_142_307_609_319_69,
            1.210_869_186_252_239_2e-6,
            0.0,
            2.413_081_956_476_048_6e-9,
        ])
    }

    pub fn shibata_882() -> Self {
        Self::new(&[
            2.075_203_657_150_268_6,
            -1.431_611_061_096_191_4,
            -4.101_862_214_156_426_5e-5,
            0.307_477_861_642_837_5,
            0.015_034_947_544_336_319,
            -0.002_069_007_372_483_611,
            -0.095_445_446_670_055_39,
            -0.017_573_365_941_643_715,
            0.001_514_684_408_903_122,
            0.009_715_720_079_839_23,
            0.003_230_015_747_249_126_4,
            -0.001_166_222_151_368_856_4,
            -0.012_702_429_667_115_211,
            -0.013_680_535_368_621_35,
            -0.000_326_957_117_067_649_96,
            -0.000_334_812_386_427_074_67,
            0.001_941_891_969_181_597_2,
            -0.006_559_844_594_448_805,
            -0.003_184_868_488_460_779,
            -0.001_185_707_631_520_927,
        ])
    }

    pub fn shibata_low_882() -> Self {
        Self::new(&[
            0.812_750_816_345_214_8,
            1.341_541_633_337_328_7e-7,
            -1.400_316_978_106_275_2e-5,
            -0.027_366_658_672_690_39,
            -0.063_084_796_071_052_55,
            -0.000_411_249_639_000_743_63,
            -0.001_466_781_133_785_843_8,
            -0.003_463_642_438_873_648_6,
            -0.014_447_951_689_362_526,
            -0.050_686_400_383_710_86,
            -0.000_316_579_535_137_861_97,
            -7.608_177_838_847_041e-7,
            1.339_193_545_391_026_4e-6,
            1.108_497_826_862_731e-6,
            2.345_899_190_459_022e-7,
            7.197_047_402_485_168e-9,
            -0.000_240_975_306_951_440_87,
            -0.000_813_391_816_336_661_6,
            -0.002_707_262_756_302_952_8,
            -1.228_902_965_522_138_4e-5,
            -2.408_082_082_183_682_4e-6,
            2.651_654_767_760_192e-6,
            0.022_208_366_543_054_58,
            1.809_095_380_167_491_4e-7,
        ])
    }

    pub fn shibata_96() -> Self {
        Self::new(&[
            2.104_111_433_029_175,
            -1.410_141_706_466_674_8,
            -0.003_514_738_753_437_996,
            0.186_179_712_414_741_52,
            0.111_176_766_455_173_49,
            -0.001_362_945_069_558_918_5,
            -0.055_446_717_888_116_84,
            -0.056_859_914_213_418_96,
            -0.003_957_323_264_330_625_5,
            0.002_566_334_791_481_495,
            0.014_090_753_160_417_08,
            0.006_225_708_406_418_562,
            -0.006_539_735_011_756_42,
            -0.019_066_527_485_847_473,
            -0.003_569_579_217_582_941,
            -0.001_226_439_489_983_022_2,
            0.000_114_401_023_893_151_43,
            -0.000_198_087_276_658_043_27,
            -0.003_230_664_879_083_633_4,
            -0.004_677_779_972_553_253,
            -0.001_040_733_186_528_086_7,
            -0.000_973_290_938_418_358_6,
            -0.000_780_345_522_798_597_8,
            -0.000_388_532_265_787_944_2,
            4.194_729_626_760_818e-5,
            0.000_172_955_406_014_807_52,
            -0.000_593_151_897_192_001_3,
            -0.000_697_247_858_624_905_3,
            -0.000_504_023_104_440_420_9,
            -0.000_376_237_061_573_192_5,
            -0.000_174_400_047_399_103_64,
            0.0,
        ])
    }

    pub fn shibata_low_96() -> Self {
        Self::new(&[
            0.833_627_820_014_953_6,
            4.766_351_082_707_842_6e-7,
            -5.592_720_481_217_839e-5,
            -0.000_917_676_079_552_620_6,
            -0.085_019_297_897_815_7,
            -0.000_308_640_970_615_670_1,
            -2.747_484_904_830_344e-5,
            -3.447_055_496_508_255_6e-5,
            -0.006_816_617_213_189_602,
            -0.005_103_240_255_266_428,
            -0.048_310_291_022_062_3,
            -3.419_442_464_291_933e-6,
            -3.938_738_757_369_72e-8,
            5.229_683_210_927_76e-6,
            2.181_512_536_481_023e-5,
            5.806_052_740_808_809e-6,
            8.897_533_007_257_152e-6,
            -2.879_307_430_703_193e-6,
            -1.014_230_292_639_695_1e-5,
            -0.000_883_434_840_943_664_3,
            -6.652_170_122_833_923e-5,
            -4.303_244_622_860_802_3e-7,
            1.557_320_956_635_521_7e-6,
            0.003_246_902_488_172_054_3,
            0.013_371_952_809_393_406,
            0.001_669_709_570_705_890_7,
            0.000_337_457_488_058_134_9,
            3.821_846_621_576_696_6e-5,
            8.088_396_134_553_477e-5,
            1.763_109_321_473_166_3e-5,
            4.731_758_963_316_679e-6,
            3.815_073_341_684_183e-7,
        ])
    }

    pub fn shibata_192() -> Self {
        Self::new(&[
            2.117_482_662_200_927_7,
            -0.793_001_294_136_047_4,
            -0.588_716_506_958_007_8,
            -0.004_517_062_101_513_147,
            -2.240_059_620_817_192e-5,
            0.349_810_659_885_406_5,
            0.001_467_469_963_245_093_8,
            -0.035_286_050_289_869_31,
            -0.030_574_915_930_628_777,
            -0.008_099_924_772_977_829,
            -0.024_920_884_519_815_445,
            -0.010_276_389_308_273_792,
            -0.002_827_338_874_340_057_4,
            0.011_965_871_788_561_344,
            -0.001_178_735_750_727_355_5,
            0.001_587_570_062_838_494_8,
            0.001_221_955_171_786_248_7,
            0.004_150_979_220_867_157,
            0.000_236_603_751_545_771_96,
            -0.000_234_691_367_950_290_44,
            0.000_245_441_042_352_467_8,
            -0.002_350_530_354_306_102,
            -0.001_063_528_005_033_731_5,
            -0.002_193_444_641_306_996_3,
            0.000_186_019_504_326_395_7,
            -0.000_534_442_078_787_833_5,
            -0.000_564_826_827_030_628_9,
            -6.555_314_757_861_197e-5,
            0.000_503_513_438_161_462_5,
            0.000_697_769_341_059_029_1,
            0.000_215_430_787_648_074_33,
            0.000_558_842_846_658_080_8,
            -0.000_955_912_342_760_711_9,
            -0.000_183_239_637_408_405_54,
            -0.001_184_734_981_507_063,
            5.595_707_625_616_342e-5,
            -0.000_210_925_907_595_083_12,
            9.261_416_380_468_29e-6,
            -1.689_312_557_573_430_2e-5,
            -0.000_102_918_987_977_318_47,
            -8.705_230_357_008_986e-6,
            -2.189_383_849_326_986_8e-5,
            2.048_334_863_502_532_2e-5,
            -9.314_835_915_574_804e-5,
            -5.457_198_494_696_058e-5,
            1.039_314_793_160_883_7e-5,
            -4.186_463_047_517_463_6e-5,
            3.314_268_178_655_766e-5,
            4.641_250_086_478_976_3e-7,
            -3.169_075_716_868_974e-5,
            2.919_960_388_680_92e-5,
            -4.137_142_968_829_721e-5,
            3.097_004_537_266_912e-6,
            -0.000_130_819_724_290_631_7,
            0.0,
            0.0,
        ])
    }

    pub fn shibata_low_192() -> Self {
        Self::new(&[
            0.929_867_863_655_090_3,
            2.375_700_432_821_759e-6,
            1.323_920_400_864_153_6e-6,
            4.533_644_570_869_91e-8,
            -1.085_569_920_178_386_4e-6,
            -7.519_394_671_362_534e-7,
            -0.010_574_714_280_664_92,
            -0.015_397_379_174_828_53,
            -0.007_173_464_633_524_418,
            -0.004_041_632_637_381_554,
            -0.000_315_436_191_158_369_2,
            -6.079_084_869_270_446e-6,
            -2.561_475_230_322_685e-5,
            -6.444_113_296_311_116e-6,
            -0.000_143_420_198_583_044_1,
            -9.988_663_229_876_238e-9,
            -0.000_110_015_651_443_973_18,
            -0.000_264_444_039_203_226_57,
            -0.018_070_342_019_200_325,
            -0.013_997_578_062_117_1,
        ])
    }

    pub fn process(&mut self, scaled: PrcFmt, dither: PrcFmt) -> PrcFmt {
        let mut filt_buf = 0.0;
        for (item, coeff) in self.buffer.iter().zip(self.filter) {
            filt_buf += coeff * item;
        }

        let scaled_plus_err = scaled + filt_buf;
        let result = scaled_plus_err + dither;
        let result_r = result.round(); // away from zero

        self.buffer.push(scaled_plus_err - result_r);

        result_r
    }
}

impl<'a> Dither<'a> {
    pub fn new<D: Ditherer + 'a>(
        name: &str,
        bits: usize,
        ditherer: D,
        shaper: Option<NoiseShaper<'a>>,
    ) -> Self {
        let name = name.to_string();
        let scalefact = PrcFmt::coerce(2.0).powi((bits - 1) as i32);
        let ditherer = Box::new(ditherer);
        Self {
            name,
            scalefact,
            ditherer,
            shaper,
        }
    }

    pub fn from_config(name: &str, conf: config::DitherParameters) -> Self {
        let (bits, shaper) = match conf {
            config::DitherParameters::None { bits } => (bits, None),
            config::DitherParameters::Flat { bits, .. } => (bits, None),
            config::DitherParameters::Highpass { bits } => (bits, None),
            config::DitherParameters::Fweighted441 { bits } => {
                (bits, Some(NoiseShaper::fweighted_441()))
            }
            config::DitherParameters::FweightedLong441 { bits } => {
                (bits, Some(NoiseShaper::fweighted_long_441()))
            }
            config::DitherParameters::FweightedShort441 { bits } => {
                (bits, Some(NoiseShaper::fweighted_short_441()))
            }
            config::DitherParameters::Gesemann441 { bits } => {
                (bits, Some(NoiseShaper::gesemann_441()))
            }
            config::DitherParameters::Gesemann48 { bits } => {
                (bits, Some(NoiseShaper::gesemann_48()))
            }
            config::DitherParameters::Lipshitz441 { bits } => {
                (bits, Some(NoiseShaper::lipshitz_441()))
            }
            config::DitherParameters::LipshitzLong441 { bits } => {
                (bits, Some(NoiseShaper::lipshitz_long_441()))
            }
            config::DitherParameters::Shibata441 { bits } => {
                (bits, Some(NoiseShaper::shibata_441()))
            }
            config::DitherParameters::ShibataHigh441 { bits } => {
                (bits, Some(NoiseShaper::shibata_high_441()))
            }
            config::DitherParameters::ShibataLow441 { bits } => {
                (bits, Some(NoiseShaper::shibata_low_441()))
            }
            config::DitherParameters::Shibata48 { bits } => (bits, Some(NoiseShaper::shibata_48())),
            config::DitherParameters::ShibataHigh48 { bits } => {
                (bits, Some(NoiseShaper::shibata_high_48()))
            }
            config::DitherParameters::ShibataLow48 { bits } => {
                (bits, Some(NoiseShaper::shibata_low_48()))
            }
            config::DitherParameters::Shibata882 { bits } => {
                (bits, Some(NoiseShaper::shibata_882()))
            }
            config::DitherParameters::ShibataLow882 { bits } => {
                (bits, Some(NoiseShaper::shibata_low_882()))
            }
            config::DitherParameters::Shibata96 { bits } => (bits, Some(NoiseShaper::shibata_96())),
            config::DitherParameters::ShibataLow96 { bits } => {
                (bits, Some(NoiseShaper::shibata_low_96()))
            }
            config::DitherParameters::Shibata192 { bits } => {
                (bits, Some(NoiseShaper::shibata_192()))
            }
            config::DitherParameters::ShibataLow192 { bits } => {
                (bits, Some(NoiseShaper::shibata_low_192()))
            }
        };

        match conf {
            config::DitherParameters::None { .. } => {
                let noop = NoopDitherer::default();
                Self::new(name, bits, noop, shaper)
            }
            config::DitherParameters::Flat { amplitude, .. } => {
                let tpdf = <TriangularDitherer as Ditherer>::new(amplitude);
                Self::new(name, bits, tpdf, shaper)
            }
            config::DitherParameters::Highpass { .. } => {
                let hp_tpdf = HighpassDitherer::default();
                Self::new(name, bits, hp_tpdf, shaper)
            }
            _ => {
                let tpdf = TriangularDitherer::default();
                Self::new(name, bits, tpdf, shaper)
            }
        }
    }
}

impl<'a> Filter for Dither<'a> {
    fn name(&self) -> &str {
        &self.name
    }

    fn process_waveform(&mut self, waveform: &mut [PrcFmt]) -> Res<()> {
        for item in waveform.iter_mut() {
            let scaled = *item * self.scalefact;
            let dither = self.ditherer.sample();

            let result_r = if let Some(shaper) = &mut self.shaper {
                shaper.process(scaled, dither)
            } else {
                let result = scaled + dither;
                result.round()
            };

            *item = result_r / self.scalefact;
        }

        Ok(())
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Dither { parameters, .. } = conf {
            *self = Self::from_config(&self.name, parameters);
        } else {
            // This should never happen unless there is a bug somewhere else
            unreachable!("Invalid config change!");
        }
    }
}

/// Validate a Dither config.
pub fn validate_config(conf: &config::DitherParameters) -> Res<()> {
    let bits = match conf {
        config::DitherParameters::None { bits }
        | config::DitherParameters::Flat { bits, .. }
        | config::DitherParameters::Highpass { bits }
        | config::DitherParameters::Fweighted441 { bits }
        | config::DitherParameters::FweightedLong441 { bits }
        | config::DitherParameters::FweightedShort441 { bits }
        | config::DitherParameters::Gesemann441 { bits }
        | config::DitherParameters::Gesemann48 { bits }
        | config::DitherParameters::Lipshitz441 { bits }
        | config::DitherParameters::LipshitzLong441 { bits }
        | config::DitherParameters::Shibata441 { bits }
        | config::DitherParameters::ShibataHigh441 { bits }
        | config::DitherParameters::ShibataLow441 { bits }
        | config::DitherParameters::Shibata48 { bits }
        | config::DitherParameters::ShibataHigh48 { bits }
        | config::DitherParameters::ShibataLow48 { bits }
        | config::DitherParameters::Shibata882 { bits }
        | config::DitherParameters::ShibataLow882 { bits }
        | config::DitherParameters::Shibata96 { bits }
        | config::DitherParameters::ShibataLow96 { bits }
        | config::DitherParameters::Shibata192 { bits }
        | config::DitherParameters::ShibataLow192 { bits } => bits,
    };
    if *bits <= 1 {
        return Err(config::ConfigError::new("Dither bit depth must be at least 2").into());
    }

    if let config::DitherParameters::Flat { amplitude, .. } = conf {
        if *amplitude < 0.0 {
            return Err(config::ConfigError::new("Dither amplitude cannot be negative").into());
        }
        if *amplitude > 100.0 {
            return Err(config::ConfigError::new("Dither amplitude must be less than 100").into());
        }
    }

    Ok(())
}

// Ditherer, TriangularDitherer, HighpassDitherer adopted from librespot,
// which is licensed under MIT. Used with permission.
pub trait Ditherer {
    // `amplitude` in bits
    fn new(amplitude: PrcFmt) -> Self
    where
        Self: Sized;

    fn sample(&mut self) -> PrcFmt;
}

// Deterministic and not cryptographically secure, but fast and with excellent
// randomness. Must be cached not only to increase performance, but more
// importantly: keep state and not repeat the same sequences.
fn create_rng() -> SmallRng {
    SmallRng::from_entropy()
}

// Spectrally-white triangular-pdf (TPDF) dither.
// The high-pass ditherer should be preferred for most audio applications.
//
// Source: Wannamaker, R.A., Lipshitz, S.P. & Vanderkooy, J. (2000).
// A Theory of Non-Subtractive Dither. University of Waterloo.
#[derive(Clone, Debug)]
pub struct TriangularDitherer {
    cached_rng: SmallRng,
    distribution: Triangular<PrcFmt>,
}

impl Ditherer for TriangularDitherer {
    fn new(amplitude: PrcFmt) -> Self {
        let amplitude = amplitude / 2.0; // negative to positive peak
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
        // 2 LSB linearizes the response.
        <Self as Ditherer>::new(2.0)
    }
}

// A very simple discrete-time noise generator capable of producing dither with
// a high-pass spectrum, and all the beneficial effects of the TPDF dither.
// Preferable in audio since it is less audible than spectrally-white TPDF
// dither. Furthermore, it is more computationally efficient since it requires
// the calculation of only one new RPDF random number per sampling period.
// This produces violet noise.
//
// Source: Wannamaker, R.A., Lipshitz, S.P. & Vanderkooy, J. (2000).
// A Theory of Non-Subtractive Dither. University of Waterloo.
#[derive(Clone, Debug)]
pub struct HighpassDitherer {
    cached_rng: SmallRng,
    previous_sample: PrcFmt,

    // optimization: makes sampling of multiple values faster
    // and with less bias than frequently calling `Rnd::gen()`.
    distribution: Uniform<PrcFmt>,
}

impl Ditherer for HighpassDitherer {
    fn new(amplitude: PrcFmt) -> Self {
        // 2x RDPF (current - previous) makes 1x TDPF
        let amplitude = amplitude / 2.0;
        Self {
            cached_rng: create_rng(),
            distribution: Uniform::new_inclusive(-amplitude, amplitude),
            previous_sample: 0.0,
        }
    }

    fn sample(&mut self) -> PrcFmt {
        let new_sample = self.distribution.sample(&mut self.cached_rng);
        let high_passed_sample = new_sample - self.previous_sample;
        self.previous_sample = new_sample;
        high_passed_sample
    }
}

impl Default for HighpassDitherer {
    fn default() -> Self {
        // 1 LSB - 1 LSB (previous) = 2 LSB
        <Self as Ditherer>::new(2.0)
    }
}

// No-op ditherer, for experimenting.
// Cheaper than checking for an `Option` in `process_waveform()`,
// especially for something that is not likely to be used very often.
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

impl Default for NoopDitherer {
    fn default() -> Self {
        <Self as Ditherer>::new(0.0)
    }
}

#[cfg(test)]
mod tests {
    use crate::{config::DitherParameters, dither::Dither, filters::Filter, PrcFmt};

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
        let conf = DitherParameters::None { bits: 8 };
        let mut dith = Dither::from_config("test", conf);
        dith.process_waveform(&mut waveform).unwrap();
        assert!(compare_waveforms(waveform.clone(), waveform2, 1.0 / 128.0));
        assert!(is_close(
            (128.0 * waveform[2]).round(),
            128.0 * waveform[2],
            1e-9
        ));
    }

    #[test]
    fn test_flat() {
        let mut waveform = vec![-1.0, -0.5, -1.0 / 3.0, 0.0, 1.0 / 3.0, 0.5, 1.0];
        let waveform2 = waveform.clone();
        let conf = DitherParameters::Flat {
            bits: 8,
            amplitude: 2.0,
        };
        let mut dith = Dither::from_config("test", conf);
        dith.process_waveform(&mut waveform).unwrap();
        assert!(compare_waveforms(waveform.clone(), waveform2, 1.0 / 64.0));
        assert!(is_close(
            (128.0 * waveform[2]).round(),
            128.0 * waveform[2],
            1e-9
        ));
    }

    #[test]
    fn test_high_pass() {
        let mut waveform = vec![-1.0, -0.5, -1.0 / 3.0, 0.0, 1.0 / 3.0, 0.5, 1.0];
        let waveform2 = waveform.clone();
        let conf = DitherParameters::Highpass { bits: 8 };
        let mut dith = Dither::from_config("test", conf);
        dith.process_waveform(&mut waveform).unwrap();
        assert!(compare_waveforms(waveform.clone(), waveform2, 1.0 / 32.0));
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
        let mut dith = Dither::from_config("test", conf);
        dith.process_waveform(&mut waveform).unwrap();
        assert!(compare_waveforms(waveform.clone(), waveform2, 1.0 / 16.0));
        assert!(is_close(
            (128.0 * waveform[2]).round(),
            128.0 * waveform[2],
            1e-9
        ));
    }
}
