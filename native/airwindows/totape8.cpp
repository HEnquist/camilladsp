// Standalone extraction of Airwindows ToTape8's double-precision DSP kernel.
// Original Copyright (c) Chris Johnson, MIT licensed.
// Source snapshot: airwindows/airwindows@781eaee378303c7dc4d9edcaabb086cf160ff5df

#include "airwindows_bridge.h"

#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>
#include <limits>
#include <new>

namespace {

constexpr double kPi = 3.141592653589793238462643383279502884;
constexpr double kPhi = 1.618033988749894848204586;

struct TapeParameters {
    std::array<double, 9> value{};

    bool valid() const noexcept {
        for (double item : value) {
            if (!std::isfinite(item) || item < 0.0 || item > 1.0) return false;
        }
        return true;
    }
};

class ToTape8 final {
public:
    explicit ToTape8(double sample_rate) noexcept : sample_rate_(sample_rate) { reset(); }

    void reset() noexcept {
        iir_enc_l_ = iir_dec_l_ = avg_enc_l_ = avg_dec_l_ = 0.0;
        iir_enc_r_ = iir_dec_r_ = avg_enc_r_ = avg_dec_r_ = 0.0;
        comp_enc_l_ = comp_dec_l_ = comp_enc_r_ = comp_dec_r_ = 1.0;
        delay_l_.fill(0.0); delay_r_.fill(0.0);
        sweep_l_ = sweep_r_ = kPi;
        next_max_l_ = next_max_r_ = 0.5;
        gcount_ = 0;
        gslew_.fill(0.0);
        iir_mid_l_ = iir_low_l_ = iir_mid_r_ = iir_low_r_ = 0.0;
        head_bump_l_ = head_bump_r_ = 0.0;
        hdb_a_.fill(0.0); hdb_b_.fill(0.0);
        last_l_ = last_r_ = 0.0;
        intermediate_l_.fill(0.0); intermediate_r_.fill(0.0);
        pos_clip_l_ = neg_clip_l_ = pos_clip_r_ = neg_clip_r_ = false;
        fpd_l_ = 0x9e3779b9U; fpd_r_ = 0x3c6ef372U;
        parameters_initialized_ = false;
        parameter_remaining_ = 0;
        derived_valid_ = false;
    }

    template <typename Sample>
    bool process(Sample *left, Sample *right, size_t frames, const double *parameters,
                 size_t transition_samples) noexcept {
        if (left == nullptr || right == nullptr || parameters == nullptr ||
            !std::isfinite(sample_rate_) || sample_rate_ <= 40000.0) return false;
        TapeParameters requested;
        std::copy_n(parameters, 9, requested.value.begin());
        if (!requested.valid()) return false;
        if (!parameters_initialized_) {
            current_ = target_ = requested;
            parameters_initialized_ = true;
            derived_valid_ = false;
        } else if (requested.value != target_.value) {
            target_ = requested;
            parameter_remaining_ = transition_samples;
            if (parameter_remaining_ == 0) {
                current_ = target_;
                derived_valid_ = false;
            }
        }

        for (size_t frame = 0; frame < frames; ++frame) {
            if (parameter_remaining_ > 0) {
                for (size_t index = 0; index < 9; ++index) {
                    current_.value[index] +=
                        (target_.value[index] - current_.value[index]) /
                        static_cast<double>(parameter_remaining_);
                }
                --parameter_remaining_;
                derived_valid_ = false;
            }
            if (!derived_valid_) update_derived();

            double input_l = static_cast<double>(left[frame]);
            double input_r = static_cast<double>(right[frame]);
            if (!std::isfinite(input_l) || !std::isfinite(input_r)) return false;
            if (std::fabs(input_l) < 1.18e-23) input_l = fpd_l_ * 1.18e-17;
            if (std::fabs(input_r) < 1.18e-23) input_r = fpd_r_ * 1.18e-17;
            input_l *= input_gain_; input_r *= input_gain_;

            input_l = dubly_encode(input_l, iir_enc_l_, avg_enc_l_, comp_enc_l_);
            input_r = dubly_encode(input_r, iir_enc_r_, avg_enc_r_, comp_enc_r_);
            flutter(input_l, input_r);
            apply_bias(input_l, input_r);

            double lows_l = 0.0, highs_l = 0.0;
            double lows_r = 0.0, highs_r = 0.0;
            tape_split(input_l, iir_mid_l_, iir_low_l_, lows_l, highs_l);
            tape_split(input_r, iir_mid_r_, iir_low_r_, lows_r, highs_r);

            double bump_l = 0.0, bump_r = 0.0;
            if (head_bump_mix_ > 0.0) {
                head_bump_l_ += lows_l * head_bump_drive_;
                head_bump_l_ -= head_bump_l_ * head_bump_l_ * head_bump_l_ * bump_damping_;
                head_bump_r_ += lows_r * head_bump_drive_;
                head_bump_r_ -= head_bump_r_ * head_bump_r_ * head_bump_r_ * bump_damping_;
                bump_l = head_biquad(head_biquad(head_bump_l_, hdb_a_, 7), hdb_b_, 7);
                bump_r = head_biquad(head_biquad(head_bump_r_, hdb_a_, 9), hdb_b_, 9);
            }
            input_l = lows_l + highs_l + bump_l * head_bump_mix_;
            input_r = lows_r + highs_r + bump_r * head_bump_mix_;
            input_l = dubly_decode(input_l, iir_dec_l_, avg_dec_l_, comp_dec_l_);
            input_r = dubly_decode(input_r, iir_dec_r_, avg_dec_r_, comp_dec_r_);
            input_l *= output_gain_; input_r *= output_gain_;
            input_l = clip_only(input_l, last_l_, intermediate_l_, pos_clip_l_, neg_clip_l_);
            input_r = clip_only(input_r, last_r_, intermediate_r_, pos_clip_r_, neg_clip_r_);
            if (!std::isfinite(input_l) || !std::isfinite(input_r)) return false;
            left[frame] = static_cast<Sample>(input_l);
            right[frame] = static_cast<Sample>(input_r);
            xorshift(fpd_l_); xorshift(fpd_r_);
        }
        return true;
    }

private:
    enum SlewIndex {
        PrevL1, PrevR1, Threshold1, PrevL2, PrevR2, Threshold2,
        PrevL3, PrevR3, Threshold3, PrevL4, PrevR4, Threshold4,
        PrevL5, PrevR5, Threshold5, PrevL6, PrevR6, Threshold6,
        PrevL7, PrevR7, Threshold7, PrevL8, PrevR8, Threshold8,
        PrevL9, PrevR9, Threshold9, SlewTotal
    };

    static void xorshift(uint32_t &value) noexcept {
        value ^= value << 13; value ^= value >> 17; value ^= value << 5;
    }

    void update_derived() noexcept {
        const auto &p = current_.value;
        overall_scale_ = sample_rate_ / 44100.0;
        spacing_ = std::clamp(static_cast<int>(std::floor(overall_scale_)), 1, 16);
        input_gain_ = std::pow(p[0] * 2.0, 2.0);
        dubly_amount_ = p[1] * 2.0;
        outly_amount_ = std::max(-1.0, (1.0 - p[1]) * -2.0);
        enc_freq_ = (1.0 - p[2]) / overall_scale_;
        dec_freq_ = p[2] / overall_scale_;
        mid_freq_ = ((p[2] * 0.618) + 0.382) / overall_scale_;
        flutter_depth_ = std::min(498.0, std::pow(p[3], 6.0) * overall_scale_ * 50.0);
        flutter_frequency_ = (0.02 * std::pow(p[4], 3.0)) / overall_scale_;
        bias_ = p[5] * 2.0 - 1.0;
        under_bias_ = (std::pow(bias_, 4.0) * 0.25) / overall_scale_;
        double over_bias = std::pow(1.0 - bias_, 3.0) / overall_scale_;
        if (bias_ > 0.0) under_bias_ = 0.0;
        if (bias_ < 0.0) over_bias = 1.0 / overall_scale_;
        for (int threshold = Threshold9; threshold >= Threshold1; threshold -= 3) {
            gslew_[threshold] = over_bias;
            over_bias *= kPhi;
        }
        head_bump_drive_ = (p[6] * 0.1) / overall_scale_;
        head_bump_mix_ = p[6] * 0.5;
        sub_freq_ = (std::sin(p[6] * kPi) * 0.008) / overall_scale_;
        bump_damping_ = 0.0618 / std::sqrt(overall_scale_);
        set_head_filter(hdb_a_, ((p[7] * p[7]) * 175.0 + 25.0) / sample_rate_);
        set_head_filter(hdb_b_, (((p[7] * p[7]) * 175.0 + 25.0) / sample_rate_) * 0.9375);
        output_gain_ = p[8] * 2.0;
        derived_valid_ = true;
    }

    static void set_head_filter(std::array<double, 11> &filter, double frequency) noexcept {
        const double resonance = 0.618033988749894848204586;
        const double k = std::tan(kPi * frequency);
        const double norm = 1.0 / (1.0 + k / resonance + k * k);
        filter[2] = k / resonance * norm;
        filter[3] = 0.0;
        filter[4] = -filter[2];
        filter[5] = 2.0 * (k * k - 1.0) * norm;
        filter[6] = (1.0 - k / resonance + k * k) * norm;
    }

    double dubly_encode(double input, double &iir, double &average, double &compressor) noexcept {
        iir = iir * (1.0 - enc_freq_) + input * enc_freq_;
        double high = (input - iir) * 2.848 + average;
        average = (input - iir) * 1.152;
        high = std::clamp(high, -1.0, 1.0);
        double amount = std::fabs(high);
        if (amount > 0.0) {
            const double adjust = std::log(1.0 + 255.0 * amount) / 2.40823996531;
            if (adjust > 0.0) amount /= adjust;
            compressor = compressor * (1.0 - enc_freq_) + amount * enc_freq_;
            input += high * compressor * dubly_amount_;
        }
        return input;
    }

    double dubly_decode(double input, double &iir, double &average, double &compressor) noexcept {
        iir = iir * (1.0 - dec_freq_) + input * dec_freq_;
        double high = (input - iir) * 2.628 + average;
        average = (input - iir) * 1.372;
        high = std::clamp(high, -1.0, 1.0);
        double amount = std::fabs(high);
        if (amount > 0.0) {
            const double adjust = std::log(1.0 + 255.0 * amount) / 2.40823996531;
            if (adjust > 0.0) amount /= adjust;
            compressor = compressor * (1.0 - dec_freq_) + amount * dec_freq_;
            input += high * compressor * outly_amount_;
        }
        return input;
    }

    void flutter(double &left, double &right) noexcept {
        if (flutter_depth_ <= 0.0) return;
        if (gcount_ < 0 || gcount_ > 999) gcount_ = 999;
        delay_l_[gcount_] = left;
        left = flutter_channel(delay_l_, gcount_, sweep_l_, sweep_r_, next_max_l_,
                               next_max_r_, fpd_l_);
        delay_r_[gcount_] = right;
        right = flutter_channel(delay_r_, gcount_, sweep_r_, sweep_l_, next_max_r_,
                                next_max_l_, fpd_r_);
        --gcount_;
    }

    double flutter_channel(std::array<double, 1002> &delay, int count, double &sweep,
                           double other_sweep, double &next_max, double other_next,
                           uint32_t &random) noexcept {
        const double offset = flutter_depth_ + flutter_depth_ * std::sin(sweep);
        sweep += next_max * flutter_frequency_;
        if (sweep > kPi * 2.0) {
            sweep -= kPi * 2.0;
            const double first = 0.24 + (random / static_cast<double>(UINT32_MAX) * 0.74);
            xorshift(random);
            const double second = 0.24 + (random / static_cast<double>(UINT32_MAX) * 0.74);
            next_max = std::fabs(first - std::sin(other_sweep + other_next)) <
                               std::fabs(second - std::sin(other_sweep + other_next))
                           ? first : second;
        }
        count += static_cast<int>(std::floor(offset));
        const double fraction = offset - std::floor(offset);
        const int first = count - (count > 999 ? 1000 : 0);
        const int second = count + 1 - (count + 1 > 999 ? 1000 : 0);
        return delay[first] * (1.0 - fraction) + delay[second] * fraction;
    }

    void apply_bias(double &left, double &right) noexcept {
        if (std::fabs(bias_) <= 0.001) return;
        for (int index = 0; index < SlewTotal; index += 3) {
            if (under_bias_ > 0.0) {
                double stuck = std::fabs(left - gslew_[index] / 0.975) / under_bias_;
                if (stuck < 1.0) left = left * stuck + (gslew_[index] / 0.975) * (1.0 - stuck);
                stuck = std::fabs(right - gslew_[index + 1] / 0.975) / under_bias_;
                if (stuck < 1.0) right = right * stuck + (gslew_[index + 1] / 0.975) * (1.0 - stuck);
            }
            left = std::clamp(left, gslew_[index] - gslew_[index + 2],
                              gslew_[index] + gslew_[index + 2]);
            gslew_[index] = left * 0.975;
            right = std::clamp(right, gslew_[index + 1] - gslew_[index + 2],
                               gslew_[index + 1] + gslew_[index + 2]);
            gslew_[index + 1] = right * 0.975;
        }
    }

    void tape_split(double input, double &mid, double &low_cut, double &lows,
                    double &highs) const noexcept {
        mid = mid * (1.0 - mid_freq_) + input * mid_freq_;
        highs = input - mid;
        lows = mid;
        if (sub_freq_ > 0.0) {
            low_cut = low_cut * (1.0 - sub_freq_) + lows * sub_freq_;
            lows -= low_cut;
        }
        lows = std::sin(std::clamp(lows, -1.57079633, 1.57079633));
        double thinned = 1.0 - std::cos(std::min(1.57079633, std::fabs(highs) * 1.57079633));
        if (highs < 0.0) thinned = -thinned;
        highs -= thinned;
    }

    static double head_biquad(double input, std::array<double, 11> &filter,
                              int state) noexcept {
        const double output = input * filter[2] + filter[state];
        filter[state] = input * filter[3] - output * filter[5] + filter[state + 1];
        filter[state + 1] = input * filter[4] - output * filter[6];
        return output;
    }

    double clip_only(double input, double &last, std::array<double, 17> &intermediate,
                     bool &positive, bool &negative) const noexcept {
        input = std::clamp(input, -4.0, 4.0);
        if (positive) last = input < last ? 0.7058208 + input * 0.2609148
                                         : 0.2491717 + last * 0.7390851;
        positive = false;
        if (input > 0.9549925859) { positive = true; input = 0.7058208 + last * 0.2609148; }
        if (negative) last = input > last ? -0.7058208 + input * 0.2609148
                                         : -0.2491717 + last * 0.7390851;
        negative = false;
        if (input < -0.9549925859) { negative = true; input = -0.7058208 + last * 0.2609148; }
        intermediate[spacing_] = input;
        const double output = last;
        for (int index = spacing_; index > 0; --index) intermediate[index - 1] = intermediate[index];
        last = intermediate[0];
        return output;
    }

    double sample_rate_;
    TapeParameters current_{}, target_{};
    bool parameters_initialized_{}, derived_valid_{};
    size_t parameter_remaining_{};
    double overall_scale_{}, input_gain_{}, dubly_amount_{}, outly_amount_{};
    double enc_freq_{}, dec_freq_{}, mid_freq_{}, flutter_depth_{}, flutter_frequency_{};
    double bias_{}, under_bias_{}, head_bump_drive_{}, head_bump_mix_{}, sub_freq_{};
    double bump_damping_{}, output_gain_{};
    int spacing_{};
    double iir_enc_l_{}, iir_dec_l_{}, comp_enc_l_{}, comp_dec_l_{}, avg_enc_l_{}, avg_dec_l_{};
    double iir_enc_r_{}, iir_dec_r_{}, comp_enc_r_{}, comp_dec_r_{}, avg_enc_r_{}, avg_dec_r_{};
    std::array<double, 1002> delay_l_{}, delay_r_{};
    double sweep_l_{}, sweep_r_{}, next_max_l_{}, next_max_r_{};
    int gcount_{};
    std::array<double, SlewTotal> gslew_{};
    double iir_mid_l_{}, iir_low_l_{}, iir_mid_r_{}, iir_low_r_{};
    double head_bump_l_{}, head_bump_r_{};
    std::array<double, 11> hdb_a_{}, hdb_b_{};
    double last_l_{}, last_r_{};
    std::array<double, 17> intermediate_l_{}, intermediate_r_{};
    bool pos_clip_l_{}, neg_clip_l_{}, pos_clip_r_{}, neg_clip_r_{};
    uint32_t fpd_l_{}, fpd_r_{};
};

} // namespace

extern "C" void *pureroad_totape8_create(double sample_rate) noexcept {
    if (!std::isfinite(sample_rate) || sample_rate <= 40000.0) return nullptr;
    return new (std::nothrow) ToTape8(sample_rate);
}

extern "C" void pureroad_totape8_destroy(void *instance) noexcept {
    delete static_cast<ToTape8 *>(instance);
}

extern "C" void pureroad_totape8_reset(void *instance) noexcept {
    if (instance != nullptr) static_cast<ToTape8 *>(instance)->reset();
}

extern "C" int pureroad_totape8_process_f64(
    void *instance, double *left, double *right, size_t frames, const double *parameters,
    size_t transition_samples) noexcept {
    return instance != nullptr && static_cast<ToTape8 *>(instance)->process(
                                      left, right, frames, parameters, transition_samples);
}

extern "C" int pureroad_totape8_process_f32(
    void *instance, float *left, float *right, size_t frames, const double *parameters,
    size_t transition_samples) noexcept {
    return instance != nullptr && static_cast<ToTape8 *>(instance)->process(
                                      left, right, frames, parameters, transition_samples);
}
