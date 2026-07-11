#ifndef PUREROAD_AIRWINDOWS_BRIDGE_H
#define PUREROAD_AIRWINDOWS_BRIDGE_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#define PUREROAD_NOEXCEPT noexcept
#else
#define PUREROAD_NOEXCEPT
#endif

void *pureroad_acceleration2_create(double sample_rate) PUREROAD_NOEXCEPT;
void pureroad_acceleration2_destroy(void *instance) PUREROAD_NOEXCEPT;
void pureroad_acceleration2_reset(void *instance) PUREROAD_NOEXCEPT;
int pureroad_acceleration2_process_f64(
    void *instance, double *left, double *right, size_t frames, double intensity,
    size_t transition_samples) PUREROAD_NOEXCEPT;
int pureroad_acceleration2_process_f32(
    void *instance, float *left, float *right, size_t frames, double intensity,
    size_t transition_samples) PUREROAD_NOEXCEPT;

void *pureroad_totape8_create(double sample_rate) PUREROAD_NOEXCEPT;
void pureroad_totape8_destroy(void *instance) PUREROAD_NOEXCEPT;
void pureroad_totape8_reset(void *instance) PUREROAD_NOEXCEPT;
int pureroad_totape8_process_f64(
    void *instance, double *left, double *right, size_t frames,
    const double *parameters, size_t transition_samples) PUREROAD_NOEXCEPT;
int pureroad_totape8_process_f32(
    void *instance, float *left, float *right, size_t frames,
    const double *parameters, size_t transition_samples) PUREROAD_NOEXCEPT;

#ifdef __cplusplus
}
#endif

#undef PUREROAD_NOEXCEPT

#endif
