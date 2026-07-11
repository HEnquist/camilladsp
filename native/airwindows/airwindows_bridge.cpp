// Standalone extraction of Airwindows Acceleration2's DSP kernel.
// Original Copyright (c) Chris Johnson, MIT licensed.
// Source snapshot: airwindows/airwindows@781eaee378303c7dc4d9edcaabb086cf160ff5df

#include "airwindows_bridge.h"

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <iterator>
#include <new>

namespace {

class Acceleration2 final {
public:
    explicit Acceleration2(double sample_rate) noexcept : sample_rate_(sample_rate) { reset(); }

    void reset() noexcept {
        std::fill(std::begin(s_l_), std::end(s_l_), 0.0);
        std::fill(std::begin(s_r_), std::end(s_r_), 0.0);
        std::fill(std::begin(biquad_a_), std::end(biquad_a_), 0.0);
        std::fill(std::begin(biquad_b_), std::end(biquad_b_), 0.0);
        set_lowpass(biquad_b_, 20000.0 / sample_rate_);
        fpd_l_ = 0x9e3779b9U;
        fpd_r_ = 0x3c6ef372U;
        parameter_initialized_ = false;
        parameter_remaining_ = 0;
        coefficient_a_ = -1.0;
        intensity_ = 0.0;
    }

    template <typename Sample>
    bool process(Sample *left, Sample *right, size_t frames, double target_a,
                 size_t transition_samples) noexcept {
        if (left == nullptr || right == nullptr || !std::isfinite(sample_rate_) ||
            sample_rate_ <= 40000.0 || !std::isfinite(target_a) || target_a < 0.0 ||
            target_a > 1.0) {
            return false;
        }

        const double overall_scale = sample_rate_ / 44100.0;
        int spacing = static_cast<int>(1.73 * overall_scale) + 1;
        spacing = std::min(spacing, 16);
        if (!parameter_initialized_) {
            current_a_ = target_a;
            target_a_ = target_a;
            parameter_initialized_ = true;
        } else if (target_a != target_a_) {
            target_a_ = target_a;
            parameter_remaining_ = transition_samples;
            if (parameter_remaining_ == 0) current_a_ = target_a_;
        }

        for (size_t frame = 0; frame < frames; ++frame) {
            if (parameter_remaining_ > 0) {
                current_a_ += (target_a_ - current_a_) /
                              static_cast<double>(parameter_remaining_);
                --parameter_remaining_;
            }
            if (current_a_ != coefficient_a_) {
                set_lowpass(biquad_a_,
                            (20000.0 *
                             (1.0 - (current_a_ * 0.618033988749894848204586))) /
                                sample_rate_);
                intensity_ = std::pow(current_a_, 3.0) * 32.0;
                coefficient_a_ = current_a_;
            }
            double input_l = static_cast<double>(left[frame]);
            double input_r = static_cast<double>(right[frame]);
            if (!std::isfinite(input_l) || !std::isfinite(input_r)) return false;
            if (std::fabs(input_l) < 1.18e-23) input_l = static_cast<double>(fpd_l_) * 1.18e-17;
            if (std::fabs(input_r) < 1.18e-23) input_r = static_cast<double>(fpd_r_) * 1.18e-17;

            const double smooth_l = biquad(input_l, biquad_a_, 7);
            const double smooth_r = biquad(input_r, biquad_a_, 9);
            for (int index = spacing * 2; index >= 0; --index) {
                s_l_[index + 1] = s_l_[index];
                s_r_[index + 1] = s_r_[index];
            }
            s_l_[0] = input_l;
            s_r_[0] = input_r;

            const double m1_l = delta_curve(s_l_[0] - s_l_[spacing]);
            const double m2_l = delta_curve(s_l_[spacing] - s_l_[spacing * 2]);
            const double sense_l =
                std::min(1.0, intensity_ * intensity_ * std::fabs(m1_l - m2_l));
            input_l = input_l * (1.0 - sense_l) + smooth_l * sense_l;

            const double m1_r = delta_curve(s_r_[0] - s_r_[spacing]);
            const double m2_r = delta_curve(s_r_[spacing] - s_r_[spacing * 2]);
            const double sense_r =
                std::min(1.0, intensity_ * intensity_ * std::fabs(m1_r - m2_r));
            input_r = input_r * (1.0 - sense_r) + smooth_r * sense_r;

            input_l = biquad(input_l, biquad_b_, 7);
            input_r = biquad(input_r, biquad_b_, 9);
            if (!std::isfinite(input_l) || !std::isfinite(input_r)) return false;
            left[frame] = static_cast<Sample>(input_l);
            right[frame] = static_cast<Sample>(input_r);
            xorshift(fpd_l_);
            xorshift(fpd_r_);
        }
        return true;
    }

private:
    static double delta_curve(double value) noexcept { return value * std::fabs(value); }

    static void xorshift(uint32_t &value) noexcept {
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
    }

    static void set_lowpass(double *filter, double normalized_frequency) noexcept {
        filter[0] = normalized_frequency;
        filter[1] = 0.7071;
        const double k = std::tan(3.141592653589793238462643383279502884 * filter[0]);
        const double norm = 1.0 / (1.0 + k / filter[1] + k * k);
        filter[2] = k * k * norm;
        filter[3] = 2.0 * filter[2];
        filter[4] = filter[2];
        filter[5] = 2.0 * (k * k - 1.0) * norm;
        filter[6] = (1.0 - k / filter[1] + k * k) * norm;
    }

    static double biquad(double input, double *filter, int state) noexcept {
        const double output = input * filter[2] + filter[state];
        filter[state] = input * filter[3] - output * filter[5] + filter[state + 1];
        filter[state + 1] = input * filter[4] - output * filter[6];
        return output;
    }

    double sample_rate_;
    double s_l_[34]{};
    double s_r_[34]{};
    double biquad_a_[11]{};
    double biquad_b_[11]{};
    uint32_t fpd_l_{};
    uint32_t fpd_r_{};
    double current_a_{};
    double target_a_{};
    double coefficient_a_{};
    double intensity_{};
    size_t parameter_remaining_{};
    bool parameter_initialized_{};
};

} // namespace

extern "C" void *pureroad_acceleration2_create(double sample_rate) noexcept {
    if (!std::isfinite(sample_rate) || sample_rate <= 40000.0) return nullptr;
    return new (std::nothrow) Acceleration2(sample_rate);
}

extern "C" void pureroad_acceleration2_destroy(void *instance) noexcept {
    delete static_cast<Acceleration2 *>(instance);
}

extern "C" void pureroad_acceleration2_reset(void *instance) noexcept {
    if (instance != nullptr) static_cast<Acceleration2 *>(instance)->reset();
}

extern "C" int pureroad_acceleration2_process_f64(
    void *instance, double *left, double *right, size_t frames, double intensity,
    size_t transition_samples) noexcept {
    return instance != nullptr &&
           static_cast<Acceleration2 *>(instance)->process(left, right, frames, intensity,
                                                           transition_samples);
}

extern "C" int pureroad_acceleration2_process_f32(
    void *instance, float *left, float *right, size_t frames, double intensity,
    size_t transition_samples) noexcept {
    return instance != nullptr &&
           static_cast<Acceleration2 *>(instance)->process(left, right, frames, intensity,
                                                           transition_samples);
}
