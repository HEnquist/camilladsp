# Building a config file step by step
Here we'll build up a full CamillaDSP config file, step by step, to help
making it easier to understand how things are connected.
This will be a simple 2-way crossover with 2 channels in and 4 out.


## Devices
First we need to define the input and output devices. Here let's assume 
we already figured out all the Loopbacks etc and already know the devices to use.
We need to decide a sample rate, let's go with 44100. For chunksize 1024 is a good values to start at with not too much delay, and low risk of buffer underruns. The best sample format this playback device supports is 32 bit integer so let's put that. The Loopback capture device supports all sample formats so let's just pick a good one.
 ```yaml
 ---
devices:
  samplerate: 44100
  chunksize: 1024
  capture:
    type: Alsa
    channels: 2
    device: "hw:Loopback,0,0"
    format: S32LE
  playback:
    type: Alsa
    channels: 4
    device: "hw:Generic_1"
    format: S32LE
```

## Mixer
We have 2 channels coming in but we need to have 4 going out. For this to work we have to add two more channels. Thus a mixer is needed. Lets name it "to4chan" and use output channels 0 & 1 for the woofers, and 2 & 3 for tweeters. Then we want to leave channels 0 & 1 as they are, and copy 0 -> 2 and 1 -> 3.
Lets start with channels 0 and 1, that should just pass through.
For each output channel we define a list of sources. Here it's a list of one.
So for each output channel X we add a section under "mapping":
```yaml
    mapping:
      - dest: X
        sources:
          - channel: Y
            gain: 0
            inverted: false
```

To copy we just need to say that output channel 0 should have channel 0 as source, with gain 0. This part becomes:
```yaml
mixers:
  to4chan:
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
```

Then we add the two new channels, by copying from channels 0 and 1: 
```yaml
mixers:
  to4chan:
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
      - dest: 2      <---- new!
        sources:
          - channel: 0
            gain: 0
            inverted: false
      - dest: 3      <---- new!
        sources:
          - channel: 1
            gain: 0
            inverted: false
```

## Pipeline
We now have all we need to build a working pipeline. It won't do any filtering yet so this is only for a quick test.
We only need a single step in the pipeline, for the "to4chan" mixer.
```yaml
pipeline:
  - type: Mixer
    name: to4chan
```
Put everything together, and run it. It should work and give unfiltered output on 4 channels.


## Filters
The poor tweeters don't like the full range signel so we need lowpass filters for them. Left and right should be filtered with the same settings, so a single definition is enough.
Let's use a simple 2nd order Butterworth at 2 kHz and name it "highpass2k". Create a "filters" section like this:
```yaml
filters:
  highpass2k:
    type: Biquad
    parameters:
      type: Highpass
      freq: 2000
      q: 0.707
```
Next we need to plug this into the pipeline after the mixer. Thus we need to extend the pipeline with two "Filter" steps, one for each tweeter channel.

```yaml
pipeline:
  - type: Mixer
    name: to4chan
  - type: Filter      <---- here!
    channel: 2
    names:
      - highpass2k
  - type: Filter      <---- here!
    channel: 3
    names:
      - highpass2k
```

When we try this we get properly filtered output for the tweeters on channels 2 and 3. Let's fix the woofers as well. Then we need a lowpass filter, so we add a definition to the filters section.
```yaml
filters:
  highpass2k:
    type: Biquad
    parameters:
      type: Highpass
      freq: 2000
      q: 0.707
  lowpass2k:
    type: Biquad
    parameters:
      type: Lowpass
      freq: 2000
      q: 0.707
```
Then we plug it into the pipeline with two new Filter blocks:
```yaml
pipeline:
  - type: Mixer
    name: to4chan
  - type: Filter
    channel: 2
    names:
      - highpass2k
  - type: Filter
    channel: 3
    names:
      - highpass2k
  - type: Filter      <---- new!
    channel: 0
    names:
      - lowpass2k
  - type: Filter      <---- new!
    channel: 1
    names:
      - lowpass2k
```

We try this and it works, but the sound isn't very nice. First off, the tweeters have higher sensitivity than the woofers, so they need to be attenuated. This can be done in the mixer, or via a separate "Gain" filter. Let's do it in the mixer, and attenuate by 5 dB. Just modify the "gain" parameters in the mixer config:
```yaml
mixers:
  to4chan:
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
            gain: -5.0      <---- here!
            inverted: false
      - dest: 3
        sources:
          - channel: 1
            gain: -5.0      <---- here!
            inverted: false
```
This is far better but we need baffle step compensation as well. We can do this with a "Highshelf" filter. The measurements say we need to attenuate by 4 dB from 500 Hz and up.
Add this filter definition:
```yaml
  bafflestep:
    type: Biquad
    parameters:
      type: Highshelf
      freq: 500
      slope: 6.0
      gain: -4.0
```
And then we plug it into the pipeline for the woofers:
```yaml
pipeline:
  - type: Mixer
    name: to4chan
  - type: Filter
    channel: 2
    names:
      - highpass2k
  - type: Filter
    channel: 3
    names:
      - highpass2k
  - type: Filter
    channel: 0
    names:
      - lowpass2k
      - bafflestep      <---- here
  - type: Filter
    channel: 1
    names:
      - lowpass2k
      - bafflestep      <---- here
```
And we are done!

## Result

```yaml
 ---
devices:
  samplerate: 44100
  chunksize: 1024
  capture:
    type: Alsa
    channels: 2
    device: "hw:Loopback,0,0"
    format: S32LE
  playback:
    type: Alsa
    channels: 4
    device: "hw:Generic_1"
    format: S32LE
    
mixers:
  to4chan:
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
            gain: -5.0
            inverted: false
      - dest: 3
        sources:
          - channel: 1
            gain: -5.0
            inverted: false

filters:
  highpass2k:
    type: Biquad
    parameters:
      type: Highpass
      freq: 2000
      q: 0.707
  lowpass2k:
    type: Biquad
    parameters:
      type: Lowpass
      freq: 2000
      q: 0.707
  bafflestep:
    type: Biquad
    parameters:
      type: Highshelf
      freq: 500
      slope: 6.0
      gain: -4.0

pipeline:
  - type: Mixer
    name: to4chan
  - type: Filter
    channel: 2
    names:
      - highpass2k
  - type: Filter
    channel: 3
    names:
      - highpass2k
  - type: Filter
    channel: 0
    names:
      - lowpass2k
      - bafflestep
  - type: Filter
    channel: 1
    names:
      - lowpass2k
      - bafflestep
```
