# Frequently asked questions

## General

- Who is Camilla?

  Camilla is my daughters middle name.

## Config files

- Why do I get a cryptic error message when my config file looks ok?
  
  In YAML it is very important that the indentation is correct, otherwise the parser is not able to deduce which properties belong to what level in the tree.
  This can result in an error message like this:
  ```
  ERRO Invalid config file!
  mapping values are not allowed in this context at line 12 column 13, module: camilladsp 
  ```
  Check the file carefully, to make sure everything is properly indented. Use only spaces, never tabs.

## Capture and playback

- Why do I get only distorted noise when using 24-bit samples?

  There are two 24-bit formats, and it's very important to pick the right one. Both use three bytes to store each sample, but they are packed in different ways.
  - S24LE: This format stores each 24-bit sample using 32 bits (4 bytes). The 24-bit data is stored in the lower three bytes, and the highest byte is padding.
    
  - S24LE3: Here only the three data bytes are stored, without any padding.

  Let's make up three samples and write them as bytes in hex. We use little-endian byte order, hence the first byte is the least significant. 
  
  Sample 1: `0xA1, 0xA2, 0xA3`, 
  
  Sample 2: `0xB1, 0xB2, 0xB3`, 
  
  Sample 3: `0xC1, 0xC2, 0xC3`  

  Stored as S24LE: `0xA1, 0xA2, 0xA3, 0x00, 0xB1, 0xB2, 0xB3, 0x00, 0xC1, 0xC2, 0xC3, 0x00` 

  Stored as S24LE3: `0xA1, 0xA2, 0xA3, 0xB1, 0xB2, 0xB3, 0xC1, 0xC2, 0xC3` 

  Note the extra padding bytes (`0x00`) in S24LE. This scheme means that the samples get an "easier" alignment in memory, while wasting some space. In practice, this format isn't used much.

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

