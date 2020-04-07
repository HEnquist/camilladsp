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

CamillaDSP includes a Websocket server that can be used to pass commands to the running process. This feature is enabled by default, but can be left out. The feature name is "socketserver". For usage see the section "Controlling via websocket".

The default FFT library is RustFFT, but it's also possible to use FFTW. This is enabled by the feature "FFTW". FFTW is about a factor two faster. It's a much larger and more complicated library though, so this is only recommended if your filters take too much CPU time with RustFFT.

### Build with standard features
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
- - with FFTW: ```cargo build --release --features FFTW```
- - combine several features: ```cargo build --release --features FFTW --features 32bit```
- The binary is now available at ./target/release/camilladsp
- Optionally install with `cargo install --path .`

### Customized build
All the available options, or "features" are:
- `alsa-backend`
- `pulse-backend`
- `socketserver`
- `FFTW`
- `32bit`

The first three (`alsa-backend`, `pulse-packend`, `socketserver`) are included in the default features, meaning if you don't specify anything you will get those three.
Cargo doesn't allow disabling a single default feature, but you can disable the whole group with the `--no-default-features` flag. Then you have to manually add all the ones you want.

Example 1: You want `alsa-backend`, `pulse-backend`, `socketserver` and `FFTW`. The first three are included by default so you only need to add `FFTW`:
```
cargo build --release --features FFTW
```

Example 2: You want `alsa-backend`, `socketserver`, `32bit` and `FFTW`. Since you don't want `pulse-backend` you have to disable the defaults, and then add both `alsa-backend` and `socketserver`:
```
cargo build --release --no-default-features --features alsa-backend --features socketserver --features FFTW --features 32bit
```


## How to run

The command is simply:
```
camilladsp /path/to/config.yml
```
This starts the processing defined in the specified config file. The config is first parsed and checked for errors. This first checks that the YAML syntax is correct, and then checks that the configuration is complete and valid. When an error is found it displays an error message describing the problem. See more about the configuration file below.

### Command line options
Starting with the --help flag prints a short help message:
```
> camilladsp --help
CamillaDSP 0.0.12
Henrik Enquist <henrik.enquist@gmail.com>
A flexible tool for processing audio

Built with features: alsa-backend, pulse-backend, socketserver

USAGE:
    camilladsp [FLAGS] [OPTIONS] <configfile>

FLAGS:
    -c, --check      Check config file and exit
    -h, --help       Prints help information
    -V, --version    Prints version information
    -v               Increase message verbosity
    -w, --wait       Wait for config from websocket

OPTIONS:
    -p, --port <port>    Port for websocket server

ARGS:
    <configfile>    The configuration file to use
```
If the "check" flag is given, the program will exit after checking the configuration file. Use this if you only want to verify that the configuration is ok, and not start any processing.

To enable the websocket server, provide a port number with the `-p` option. Leave it out, or give 0 to disable. 

If the "wait" flag, `-w` is given, CamillaDSP will start the websocket server and wait for a configuration to be uploaded. Then the config file argument must be left out.

The default logging setting prints messages of levels "error", "warn" and "info". By passing the verbosity flag once, `-v` it also prints "debug". If and if's given twice, `-vv`, it also prints "trace" messages. 


### Reloading the configuration
The configuration can be reloaded without restarting by sending a SIGHUP to the camilladsp process. This will reload the config and if possible apply the new settings without interrupting the processing. Note that for this to update the coefficients for a FIR filter, the filename of the coefficients file needs to change.

## Controlling via websocket
See the [separate readme for the websocket server](./websocket.md)

If the websocket server is enabled with the -p option, CamillaDSP will listen to incoming websocket connections on the specified port.



## Usage example: crossover for 2-way speakers
A crossover must filter all sound being played on the system. This is possible with both PulseAudio and Alsa by setting up a loopback device (Alsa) or null sink (Pulse) and setting this device as the default output device. CamillaDSP is then configured to capture from the output of this device and play the processed audio on the real sound card.

See the [tutorial for a step-by-step guide.](./stepbystep.md)


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

## The YAML format
CamillaDSP is using the YAML format for the configuration file. This is a standard format that was chosen because of its nice readable syntax. The Serde library is used for reading the configuration. 
There are a few things to keep in mind with YAML. The configuration is a tree, and the level is determined by the indentation level. For YAML the indentation is as important as opening and closing brackets in other formats. If it's wrong, Serde might not be able to give a good description of what the error is, only that the file is invalid. 
If you get strange errors, first check that the indentation is correct. Also check that you only use spaces and no tabs. Many text editors can help by highlighting syntax errors in the file. 

## Devices
Example config (note that parameters marked (*) can be left out to use their default values):
```
devices:
  samplerate: 44100
  chunksize: 1024
  queuelimit: 128 (*)
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
* `samplerate`

  The `samplerate` setting decides the sample rate that everything will run at. 
  This rate must be supported by both the capture and  playback device.

* `chunksize`

  All processing is done in chunks of data. The `chunksize` is the number of samples each chunk will have per channel. 
  It's good if the number is an "easy" number like a power of two, since this speeds up the FFT in the Convolution filter. 
  A good value to start at is 1024. 
  If you have long FIR filters you can make this larger to reduce CPU usage. 
  Try increasing in factors of two, to 2048, 4096 etc. 
  The duration in seconds of a chunk is `chunksize/samplerate`, so a value of 1024 at 44.1kHz corresponds to 23 ms per chunk.

* `queuelimit` (optional)

  The field `queuelimit` should normally be left out to use the default of 128. 
  It sets the limit for the length of the queues between the capture device and the processing thread, 
  and between the processing thread and the playback device. 
  The total queue size limit will be `2*chunksize*queuelimit` samples per channel. 
  The maximum RAM usage is `8*2*chunksize*queuelimit` bytes. 
  For example at the default setting of 128 and a chunksize of 1024, the total size limit of the queues 
  is about 2MB (or 1MB if the 32bit compile option is used). 
  The queues are allocated as needed, this value only sets an upper limit. 

  The value should only be changed if the capture device provides data faster 
  than the playback device can play it. 
  This will only be the case when piping data in via the file capture device, 
  and will lead to very high cpu usage while the queues are being filled. 
  If this is a problem, set `queuelimit` to a low value like 1.

* `target_level` & `adjust_period` (optional)
  For the special case where the capture device is an Alsa Loopback device, 
  and the playback device another Alsa device, there is a function to synchronize 
  the Loopback device to the playback device. 
  This avoids the problems of buffer underruns or slowly increasing delay. 
  This function requires the parameter `target_level` to be set. 
  The value is the number of samples that should be left in the buffer of the playback device
  when the next chunk arrives. It works by fine tuning the sample rate of the virtual Loopback device.
  It will take some experimentation to find the right number. 
  If it's too small there will be buffer underruns from time to time, 
  and making it too large might lead to a longer input-output delay than what is acceptable. 
  Suitable values are in the range 1/2 to 1 times the `chunksize`. 

  The `adjust_period` parameter is used to set the interval between corrections. 
  The default is 10 seconds.

* `silence_threshold` & `silence_timeout` (optional)
  The fields `silence_threshold` and `silence_timeout` are optional 
  and used to pause processing if the input is silent. 
  The threshold is the threshold level in dB, and the level is calculated as the difference 
  between the minimum and maximum sample values for all channels in the capture buffer. 
  0 dB is full level. Some experimentation might be needed to find the right threshold.

  The `silence_timeout` (in seconds) is for how long the signal should be silent before pausing processing. 
  Set this to zero, or leave it out, to never pause.
 
* `capture` and `playback`
  Input and output devices are defined in the same way. 
  A device needs:
  * `type`: Alsa, Pulse or File 
  * `channels`: number of channels
  * `device`: device name (for Alsa and Pulse)
  * `filename` path the the file (for File)
  * `format`: sample format.

    Currently supported sample formats are signed little-endian integers of 16, 24 and 32 bits as well as floats of 32 and 64 bits:
    * S16LE
    * S24LE
    * S32LE 
    * FLOAT32LE
    * FLOAT64LE (not supported by PulseAudio)

  The File device type reads or writes to a file. 
  The format is raw interleaved samples, 2 bytes per sample for 16-bit, 
  and 4 bytes per sample for 24 and 32 bits. 
  If the capture device reaches the end of a file, the program will exit once all chunks have been played. 
  That delayed sound that would end up in a later chunk will be cut off. To avoid this, set the optional parameter `extra_samples` for the File capture device.
  This causes the capture device to yield the given number of samples (rounded up to a number of complete chunks) after reaching end of file, allowing any delayed sound to be played back.
  By setting the filename to `/dev/stdin` for capture, or `/dev/stdout` for playback, the sound will be written to or read from stdio, so one can play with pipes:
  ```
  > camilladsp stdio_capt.yml > rawfile.dat
  > cat rawfile.dat | camilladsp stdio_pb.yml
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
A FIR filter is given by an impulse response provided as a list of coefficients. The coefficients are preferably given in a separate file, but can be included directly in the config file. If the number of coefficients (or taps) is larger than the chunksize setting it will use segmented convolution. The number of segments is the filter length divided by the chunksize, rounded up.
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
IIR filters are Biquad filters. CamillaDSP can calculate the coefficients for a number of standard filters, or you can provide the coefficients directly.
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
  LR_highpass:
    type: BiquadCombo
    parameters:
      type: LinkwitzRileyHighpass
      freq: 1000
      order: 4
```

Single Biquads are defined using the type "Biquad". The available filter types are:
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

To build more complex filters, use the type "BiquadCombo". This automatically adds several Biquads to build other filter types. The available types are:
* ButterworthHighpass & ButterworthLowpass
  * defined by frequency, `freq` and filter `order`.
* LinkwitzRileyHighpass & LinkwitzRileyLowpass
  * defined by frequency, `freq` and filter `order`.
  * Note, the order must be even

Other types such as Bessel filters can be built by combining several Biquads. [See the separate readme for more filter functions.](./filterfunctions.md)


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
- Simple, simple noise shaping with increasing noise towards higher frequencies
- Uniform, just dither, no shaping. Requires also the parameter "amplitude" to set the dither amplitude in bits.
- Lipshitz441, for 44.1 kHz
- Fweighted441, for 44.1 kHz
- Shibata441, for 44.1 kHz
- Shibata48, for 48 kHz
- None, just quantize without dither. Only useful with small target bit depth for demonstration.

Lipshitz, Fweighted and Shibata give the least amount ofaudible noise. [See the SOX documentation for more details.](http://sox.sourceforge.net/SoX/NoiseShaping)
To test the different types, set the target bit depth to something very small like 5 bits and try them all.


### Difference equation
The "DiffEq" filter implements a generic difference equation filter with transfer function:
H(z) = (b0 + b1*z^-1 + .. + bn*z^-n)/(a0 + a1*z^-1 + .. + an*z^-n). The coefficients are given as a list a0..an in that order. Example:
```
  example_diffeq:
    type: DiffEq
    parameters:
      a: [1.0, -0.1462978543780541, 0.005350765548905586]
      b: [0.21476322779271284, 0.4295264555854257, 0.21476322779271284]
```
This example implements a Biquad lowpass, but for a Biquad the Free Biquad type is faster and should be preferred. Both a and b are optional. If left out, they default to [1.0].


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
