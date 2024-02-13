# Building a config file step by step
Here we'll build up a full CamillaDSP config file, step by step, to help
making it easier to understand how things are connected.
This will be a simple 2-way crossover with 2 channels in and 4 out.


## Devices
First we need to define the input and output devices. Here let's assume
we already figured out all the Loopbacks etc and already know the devices to use.
We need to decide a sample rate, let's go with 44100.
For chunksize, 1024 is a good starting point.
This gives a fairly short delay, and low risk of buffer underruns.
The best sample format this playback device supports is 32 bit integer so let's put that.
The Loopback capture device supports all sample formats so let's just pick a good one.
```yaml
---
title: "Example crossover"
description: "An example of a simple 2-way crossover"

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
We have 2 channels coming in but we need to have 4 going out.
For this to work we have to add two more channels. Thus a mixer is needed.
Lets name it "to4chan" and use output channels 0 & 1 for the woofers, and 2 & 3 for tweeters.
Then we want to leave channels 0 & 1 as they are, and copy 0 -> 2 and 1 -> 3.
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

To copy we just need to say that output channel 0 should have channel 0 as source, with gain 0.
This part becomes:
```yaml
mixers:
  to4chan:
    description: "Expand 2 channels to 4"
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
    description: "Expand 2 channels to 4"
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
We now have all we need to build a working pipeline.
It won't do any filtering yet so this is only for a quick test.
We only need a single step in the pipeline, for the "to4chan" mixer.
```yaml
pipeline:
  - type: Mixer
    name: to4chan
```
Put everything together, and run it. It should work and give unfiltered output on 4 channels.


## Filters
The poor tweeters don't like the full range signal so we need lowpass filters for them.
Left and right should be filtered with the same settings, so a single definition is enough.
Let's use a simple 2nd order Butterworth at 2 kHz and name it "highpass2k".

Create a "filters" section like this:
```yaml
filters:
  highpass2k:
    type: Biquad
    parameters:
      type: Highpass
      freq: 2000
      q: 0.707
```
Next we need to plug this into the pipeline after the mixer.
Thus we need to extend the pipeline with a "Filter" step,
that acts on the two tweeter channels.

```yaml
pipeline:
  - type: Mixer
    name: to4chan
  - type: Filter      <---- here!
    channels: [2, 3]
    names:
      - highpass2k
```

When we try this we get properly filtered output for the tweeters on channels 2 and 3.
Let's fix the woofers as well.
Then we need a lowpass filter, so we add a definition to the filters section.
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

Then we plug the woofer filter into the pipeline with a new Filter block:
```yaml
pipeline:
  - type: Mixer
    name: to4chan
  - type: Filter
    channels: [2, 3]
    names:
      - highpass2k
  - type: Filter      <---- new!
    channels: [0, 1]
    names:
      - lowpass2k
```

We try this and it works, but the sound isn't very nice.
First off, the tweeters have higher sensitivity than the woofers, so they need to be attenuated.
This can be done in the mixer, or via a separate "Gain" filter.
Let's do it in the mixer, and attenuate by 5 dB.

Just modify the "gain" parameters in the mixer config:
```yaml
mixers:
  to4chan:
    description: "Expand 2 channels to 4"
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
This is far better but we need baffle step compensation as well.
We can do this with a "Highshelf" filter.
The measurements say we need to attenuate by 4 dB from 500 Hz and up.

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
The baffle step correction should be applied to both woofers and tweeters,
so let's add this in a new Filter step before the Mixer:
```yaml
pipeline:
  - type: Filter      \
    channels: [0, 1]  |  <---- new
    names:            |
      - bafflestep    /
  - type: Mixer
    name: to4chan
  - type: Filter
    channels: [2, 3]
    names:
      - highpass2k
  - type: Filter
    channels: [0, 1]
    names:
      - lowpass2k
```
The last thing we need to do is to adjust the delay between tweeter and woofer.
Measurements tell us we need to delay the tweeter by 0.5 ms.

Add this filter definition:
```yaml
  tweeterdelay:
    type: Delay
    parameters:
      delay: 0.5
      unit: ms
```

Now we add this to the tweeter channels:
```yaml
pipeline:
  - type: Filter
    channels: [0, 1]
    names:
      - bafflestep
  - type: Mixer
    name: to4chan
  - type: Filter
    channels: [2, 3]
    names:
      - highpass2k
      - tweeterdelay      <---- here!
  - type: Filter
    channels: [0, 1]
    names:
      - lowpass2k
```
And we are done!

## Result

Now we have all the parts of the configuration.
As a final touch, let's add descriptions to all pipeline steps
while we have things fresh in memory.

```yaml
---
title: "Example crossover"
description: "An example of a simple 2-way crossover"

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
    description: "Expand 2 channels to 4"
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
    description: "2nd order highpass crossover"
    parameters:
      type: Highpass
      freq: 2000
      q: 0.707
  lowpass2k:
    type: Biquad
    description: "2nd order lowpass crossover"
    parameters:
      type: Lowpass
      freq: 2000
      q: 0.707
  bafflestep:
    type: Biquad
    description: "Baffle step compensation"
    parameters:
      type: Highshelf
      freq: 500
      slope: 6.0
      gain: -4.0
  tweeterdelay:
    type: Delay
    description: "Time alignment for tweeters"
    parameters:
      delay: 0.5
      unit: ms

pipeline:
  - type: Filter
    description: "Pre-mixer filters"
    channela: [0, 1]
    names:
      - bafflestep
  - type: Mixer
    name: to4chan
  - type: Filter
    description: "Highpass for tweeters"
    channels: [2, 3]
    names:
      - highpass2k
      - tweeterdelay
  - type: Filter
    description: "Lowpass for woofers"
    channels: [0, 1]
    names:
      - lowpass2k
```
