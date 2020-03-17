# CamillaDSP
![CI test and lint](https://github.com/HEnquist/camilladsp/workflows/CI%20test%20and%20lint/badge.svg)

A tool to create audio processing pipelines for applications such as active crossovers or room correction. It is written in Rust to benefit from the safety and elegant handling of threading that this language provides. 

Audio data is captured from a capture device and sent to a playback device. Alsa and PulseAudio are currently supported for both capture and playback.

The processing pipeline consists of any number of filters and mixers. Mixers are used to route audio between channels and to change the number of channels in the stream. Filters can be both IIR and FIR. IIR filters are implemented as biquads, while FIR use convolution via FFT/IFFT. A filter can be applied to any number of channels. All processing is done in chunks of a fixed number of samples. A small number of samples gives a small in-out latency while a larger number is required for long FIR filters.
The full configuration is given in a yaml file.

### Background
The purpose of CamillaDSP is to enable audio processing with combinations of FIR and IIR filters. This functionality is available in EqualizerAPO, but for Windows only. For Linux the best known FIR filter engine is probably BruteFIR, which works very well but doesn't support IIR filters.

* BruteFIR: https://www.ludd.ltu.se/~torger/brutefir.html
* EqualizerAPO: https://sourceforge.net/projects/equalizerapo/
* The IIR filtering is heavily inspired by biquad-rs: https://github.com/korken89/biquad-rs 

### Dependencies
* https://crates.io/crates/rustfft - FFT used for FIR filters
* https://crates.io/crates/libpulse-simple-binding - PulseAudio audio backend 
* https://crates.io/crates/alsa - Alsa audio backend
* https://crates.io/crates/serde_yaml - Config file reading
* https://crates.io/crates/num-traits - Converting between number types

## Building

Use recent stable versions of rustc and cargo. The minimum rustc version is 1.36.0.

By default both the Alsa and PulseAudio backends are enabled, but they can be disabled if desired. That also removes the need for the the corresponding system Alsa/Pulse packages.

By default the internal processing is done using 64-bit floats. There is a possibility to switch this to 32-bit floats. This might be useful for speeding up the processing when running on a 32-bit CPU (or a 64-bit CPU running in 32-bit mode), but the actual speed advantage has not been evaluated. Note that the reduction in precision increases the numerical noise.
- Install pkg-config (very likely already installed):
- - Fedora: ```sudo dnf install pkgconf-pkg-config```
- - Debian/Ubuntu etc: ```sudo apt-get install pkg-config```
- Install Alsa dependency:
- - Fedora: ```sudo dnf install alsa-lib-devel```
- - Debian/Ubuntu etc: ```sudo apt-get install libasound2-dev```
- Install Pulse dependency:
- - Fedora: ```sudo dnf install pulseaudio-libs-devel```
- - Debian/Ubuntu etc: ```sudo apt-get install libpulse-dev```
- Clone the repository
- Build with standard options: ```cargo build --release```
- - without Alsa: ```cargo build --release --no-default-features --features pulse-backend```
- - without Pulse: ```cargo build --release --no-default-features --features alsa-backend```
- - with 32 bit float: ```cargo build --release --features 32bit```
- The binary is now available at ./target/release/camilladsp
- Optionally install with `cargo install --path .`

## How to run

The command is simply:
```
camilladsp /path/to/config.yml
```
This starts the processing defined in the specified config file. The config is first parsed and checked for errors. This first checks that the YAML syntax is correct, and then checks that the configuration is complete and valid. When an error is found it displays an error message describing the problem.

### Command line options
Starting with the --help flag prints a short help message:
```
> camilladsp --help
CamillaDSP 0.0.9
Henrik Enquist <henrik.enquist@gmail.com>
A flexible tool for processing audio

USAGE:
    camilladsp [FLAGS] <configfile>

FLAGS:
    -c, --check      Check config file and exit
    -h, --help       Prints help information
    -V, --version    Prints version information
    -v               Increase message verbosity

ARGS:
    <configfile>    The configuration file to use
```
If the "check" flag is given, the program will exit after checking the configuration file. Use this if you only want to verify that the configuration is ok, and not start any processing.

The default logging setting prints messeges of levels "error", "warn" and "info". By passing the verbosity flag once, "-v" it also prints "debug". If and if's given twice, "-vv", it also prints "trace" messages. 


### Reloading the configuration
The configuration can be reloaded without restarting by sending a SIGHUP to the camilladsp process. This will reload the config and if possible apply the new settings without interrupting the processing. Note that for this to update the coefficients for a FIR filter, the filename of the coefficents file needs to change.


## Usage example: crossover for 2-way speakers
A crossover must filter all sound being played on the system. This is possible with both PulseAudio and Alsa by setting up a loopback device (Alsa) or null sink (Pulse) and setting this device as the default output device. CamillaDSP is then configured to capture from the output of this device and play the processed audio on the real sound card.
The simplest possible processing pipeline would then consist of:
- A source, for example a Pulse null sink
- A Mixer to go from 2 to four channels (2 for woofers, 2 for tweeters)
- High pass filters on the tweeter channels
- Low pass filter on the woofer channels
- An output device with 4 analog channel


# Capturing audio
In order to insert CamillaDSP between applications and the sound card, a virtual sound card is required. This works with both Alsa and PulseAudio.
## Alsa
An Alsa Loopback device can be used. This device behaves like a sound card with two devices playback and capture. The sound being send to the playback side on one device can then be captured from the capture side on the other device. To load the kernel device type:
```
sudo modprobe snd-aloop
```
Find the name of the device:
```
aplay -l
```
Play a track on card 2, device 1, subdevice 0 (the audio can then be captured from card 2, device 0, subdevice 0):
```
aplay -D hw:2,1,0 sometrack.wav
```

## PulseAudio
PulseAudio provides a null-sink that can be used to capture audio from applications. To create a null sink type:
```
pacmd load-module module-null-sink sink_name=MySink
```
This device can be set as the default output, meaning any application using PulseAudio will use it. The audio sent to this device can then be captured from the monitor output named MySink.monitor.
All available sinks and sources can be listed with the commands:
```
pacmd list-sinks
pacmd list-sources
```


# Configuration

## Devices
Input and output devices are defined in the same way. A device needs a type (Alsa, Pulse or File), number of channels, a device name (or file name for File), and a sample format. Currently supported sample formats are signed little-endian integers of 16, 24 and 32 bits (S16LE, S24LE and S32LE) as well as floats of 32 and 64 bits (FLOAT32LE and FLOAT64LE). Note that PulseAudio does not support 64-bit float. 
There is also a common samplerate that decides the samplerate that everything will run at. The buffersize is the number of samples each chunk will have per channel. 
The fields silence_threshold and silence_timeout are optional and used to pause processing if the input is silent. The threshold is the threshold level in dB, and the level is calculated as the difference between the minimum and maximum sample values for all channels in the capture buffer. 0 dB is full level. Some experimentation might be needed to find the right threshold.
The timeout (in seconds) is for how long the signal should be silent before pausing processing. Set this to zero, or leave it out, to never pause.
For the special case where the capture device is an Alsa Loopback device, and the playback device another Alsa device, there is a funtion to synchronize the Loopback device to the playback device. This avoids the problems of buffer underruns or slowly increasing delay. This function requires the parameter "target_level" to be set. It will take some experimentation to find the right number. If it's too small there will be buffer underruns from time to time, and making it too large might lead to a longer input-output delay than what is acceptable. Suitable values are in the range 1/2 to 1 times the buffersize. The "adjust_period" sets the interval between corrections and is optional. The default is 10 seconds.

Example config (note that parameters marked (*) can be left out to use their default values):
```
devices:
  samplerate: 44100
  buffersize: 1024
  silence_threshold: -60 (*)
  silence_timeout: 3.0 (*)
  target_level: 500 (*)
  adjust_period: 10 (*)
  capture:
    type: Pulse
    channels: 2
    device: "MySink.monitor"
    format: S16LE
  playback:
    type: Alsa
    channels: 2
    device: "hw:Generic_1"
    format: S32LE
```

The File device type reads or writes to a file. The format is raw interleaved samples, 2 bytes per sample for 16-bit, and 4 bytes per sample for 24 and 32 bits. If the capture device reaches the end of a file, the program will exit once all chunks have been played. Note that delayed sound that would end up in a later chunk will be cut off. By setting the filename to ```/dev/stdin``` for capture, or ```/dev/stdout``` for playback, the sound will be written to or read from stdio, so one can play with pipes:
```
camilladsp stdio_capt.yml > rawfile.dat
cat rawfile.dat | camilladsp stdio_pb.yml
```


## Mixers
A mixer is used to route audio between channels, and to increase or decrease the number of channels in the pipeline.
Example for a mixer that copies two channels into four:
```
mixers:
  ExampleMixer:
    channels:
      in: 2
      out: 4
    mapping:
      - dest: 0
        sources:
          - channel: 0
            gain: 0
            inverted: false
      - dest: 1
        sources:
          - channel: 1
            gain: 0
            inverted: false
      - dest: 2
        sources:
          - channel: 0
            gain: 0
            inverted: false
      - dest: 3
        sources:
          - channel: 1
            gain: 0
            inverted: false
```
The "channels" group define the number of input and output channels for the mixer. The mapping section then decides how to route the audio.
This is a list of the output channels, and for each channel there is a "sources" list that gives the sources for this particular channel. Each source has a channel number, a gain value in dB, and if it should be inverted (true/false). A channel that has no sources will be filled with silence.
Another example, a simple stereo to mono mixer:
```
mixers:
  mono:
    channels:
      in: 2
      out: 1
    mapping:
      - dest: 0
        sources:
          - channel: 0
            gain: -6
            inverted: false
          - channel: 1
            gain: -6
            inverted: false
```

## Filters
The filters section defines the filter configurations to use in the pipeline. It's enough to define each filter once even if it should be applied on several channels.
The supported filter types are Biquad for IIR and Conv for FIR. There are also filters just providing gain and delay. The last filter type is Dither, which is used to add dither when quantizing the output.

### Gain
The gain filter simply changes the amplitude of the signal. The "inverted" parameter simply inverts the signal. This parameter is optional and the default is to not invert.
```
filters:
  gainexample:
    type: Gain
    parameters:
      gain: -6.0 
      inverted: false
```

### Delay
The delay filter provides a delay in milliseconds or samples. The "unit" can be "ms" or "samples", and if left out it defaults to "ms". The millisecond value will be rounded to the nearest number of samples.
```
filters:
  delayexample:
    type: Delay
    parameters:
      delay: 12.3
      unit: ms
```

### FIR
A FIR filter is given by an impuse response provided as a list of coefficients. The coefficients are preferrably given in a separate file, but can be included directly in the config file. If the number of coefficients (or taps) is larger than the buffersize setting it will use segmented convolution. The number of segments is the filter length divided by the buffersize, rounded up.
```
filters:
  lowpass_fir:
    type: Conv
    parameters:
      type: File 
      filename: path/to/filter.txt
      format: TEXT
```
For testing purposes the entire "parameters" block can be left out (or commented out with a # at the start of each line). This then becomes a dummy filter that does not affect the signal.
The "format" parameter can be omitted, in which case it's assumed that the format is TEXT. This format is a simple text file with one value per row:
```
-0.000021
-0.000020
-0.000018
...
-0.000012
```
The other possible formats are raw data:
- S16LE: signed 16 bit little-endian integers
- S24LE: signed 24 bit little-endian integers stored as 32 bits (with the data in the low 24)
- S32LE: signed 32 bit little-endian integers
- FLOAT32LE: 32 bit little endian float
- FLOAT64LE: 64 bit little endian float


### IIR
IIR filters are Biquad filters. CamillaDSP can calculate the coefficients for a number of standard filter, or you can provide the coefficients directly.
Examples:
```
filters:
  free_nbr1:
    type: Biquad
    parameters:
      type: Free
      a1: 1.0
      a2: 1.0
      b0: 1.0
      b1: 1.0
      b2: 1.0
  hp_80:
    type: Biquad
    parameters:
      type: Highpass
      freq: 80
      q: 0.5
  peak_100:
    type: Biquad
    parameters:
      type: Peaking
      freq: 100
      q: 0.5
      gain: -7.3
  exampleshelf:
    type: Biquad
    parameters:
      type: Highshelf
      freq: 1000
      slope: 6
      gain: -12
```

The available types are:
* Free
  * given by normalized coefficients a1, a2, b0, b1, b2.
* Highpass & Lowpass
  * Second order high/lowpass filters (12dB/oct)
  * Defined by cutoff frequency and Q-value
* HighpassFO & LowpassFO
  * First order high/lowpass filters (6dB/oct)
  * Defined by cutoff frequency.
* Highshelf & Lowshelf
  * High / Low uniformly affects the high / low frequencies respectively while leaving the low / high part unaffected. In between there is a slope of variable steepness.
  * "gain" gives the gain of the filter
  * "slope" is the steepness in dB/octave. Values up to around +-12 are usable.
  * "freq" is the center frequency of the sloping section.
* Peaking
  * A parametric peaking filter with selectable gain af a given frequency with a bandwidth given by the Q-value.
* Notch
  * A notch filter to attenuate a given frequency with a bandwidth given by the Q-value.
* Bandpass
  * A second order bandpass filter for a given frequency with a bandwidth given by the Q-value. 
* Allpass
  * A second order allpass filter for a given frequency with a steepness given by the Q-value. 
* LinkwitzTransform
  * A Linkwitz transform to change a speaker with resonance frequency ```freq_act``` and Q-value ```q_act```, to a new resonance frequency ```freq_target``` and Q-value ```q_target```.

### Dither
The "Dither" filter should only be added at the very end of the pipeline for each channel, and adds noise shaped dither to the output. This is intended for 16-bit output, but can be used also for higher bit depth if desired. There are several types, and the parameter "bits" sets the target bit depth. This should match the bit depth of the playback device. Example:
```
  dither_fancy:
    type: Dither
    parameters:
      type: Lipshitz
      bits: 16
```
The available types are 
- Simple, simple noise shaping with increasing noise towards higher freqencies
- Uniform, just digher, no shaping. Requires also the parameter "amplitude" to set the dither amplitude in bits.
- Lipshitz, intended for 44.1 kHz, gives very little subjective noise
- None, just quantize without dither. Only useful with small target bit depth for demonstation.

To test the differenc types, set the target bit depth to something very small like 5 bits and try them all.


## Pipeline
The pipeline section defines the processing steps between input and output. The input and output devices are automatically added to the start and end. 
The pipeline is essentially a list of filters and/or mixers. There are no rules for ordering or how many are added. For each mixer and for the output device the number of channels from the previous step must match the number of input channels.
Example:
```
pipeline:
  - type: Mixer
    name: to4channels
  - type: Filter
    channel: 0
    names:
      - lowpass_fir
      - peak1
  - type: Filter
    channel: 1
    names:
      - lowpass_fir
      - peak1
  - type: Filter
    channel: 2
    names:
      - highpass_fir
  - type: Filter
    channel: 3
    names:
      - highpass_fir
```
In this config first a mixer is used to copy a stereo input to four channels. Then for each channel a filter step is added. A filter block can contain one or several filters that must be define in the "Filters" section. Here channel 0 and 1 get filtered by "lowpass_fir" and "peak1", while 2 and 3 get filtered by just "highpass_fir". 

## Visualizing the config
A Python script is included to view the configuration. This plots the transfer functions of all included filters, as well as plots a flowchart of the entire processing pipeline. Run it with:
```
python show_config.py /path/to/config.yml
```

Example flowchart:

![Example](pipeline.png)

Note that the script assumes a valid configuration file and will not give any helpful error messages if it's not, so it's a good idea to first use CamillaDSP to validate the file.
The script requires the following:
* Python 3
* Numpy
* Matplotlib
* PyYAML
