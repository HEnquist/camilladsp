# Frequently asked questions

## Filtering

- I only have filters with negative gain, why do I get clipping anyway?
  
  If all filters have negative gain, then the 
  It's not very intuitive, but the peak amplitude can actually increase when you apply filters that only attenuate. 
  
  The signal is a sum of a large number of frequency components, and in each particular sample some components 
  will add to increase the amplitude while other decrease it. 
  If a filter happens to remove a component that lowers the amplitude in a sample, then the value here will go up. 
  Also all filters affect the phase in a wide range, and this also makes the components sum up to a new waveform that can have higher peaks.
  This is mostly a problem with modern productions that are already a bit clipped to begin with, meaning they have many samples at max amplitude. 
  Try adding a -3 dB Gain filter, that should be enough in most cases.

- When do I need to use an asynchronous resampler?

  The asynchronous resampler must be used when the ratio between the input and output sample rates cannot be expressed as a fixed ratio.
  This is only the case when resampling to adaptively match the rate of two devices with independant clocks, where the ratio drifts a little all the time.
  Note that resampling between the fixed rates 44.1 kHz -> 48 kHz corresponds to a ratio of 160/147, and can be handled by the synchronous resampler.
  This works for any fixed resampling between the standard rates, 44.1 <-> 96 kHz, 88.2 <-> 192 kHz, 88.1 <-> 48 kHz etc.

- My impulse response is a wav-file. How to I use it in CamillaDSP?

  The wav-file must be converted to raw format, with one file per channel. Please see the [guide for converting](coefficients_from_wav.md).

