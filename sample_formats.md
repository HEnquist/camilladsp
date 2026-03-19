# Sample formats

| Format                      | Type       | Bits     | Bytes | Byte order    | Storage |
|-----------------------------|------------|----------|-------|---------------|---------|
| [S16_LE](#s16_le)           | Integer    | 16       | 2     | little-endian | packed  |
| [S24_3_LE](#s24_3_le)       | Integer    | 24       | 3     | little-endian | packed  |
| [S24_4_LJ_LE](#s24_4_lj_le) | Integer    | 24       | 4     | little-endian | padded, left justified  |
| [S24_4_RJ_LE](#s24_4_rj_le) | Integer    | 24       | 4     | little-endian | padded, right justified |
| [S32_LE](#s32_le)           | Integer    | 32       | 4     | little-endian | packed  |
| [F32_LE](#f32_le)           | Float      | 32       | 4     | little-endian | packed  |
| [F64_LE](#f64_le)           | Float      | 64       | 8     | little-endian | packed  |


## Byte order
For all sample formats with more than 8 bits per sample,
more than one byte is required to store each sample.
When storing these bytes in memory or in a file,
there is a choice of what order to place the bytes in.
The most common byte order is called *little-endian*,
where the least significant byte is stored in the smallest (first) address.
*Big-endian* is the opposite, where the most significant byte
is stored in the smallest (first) address.

In practice, big-endian formats are very rarely used.
CamillaDSP therefore only supports samples stored in little-endian byte order.

## Storage
Audio data in sample formats where the number of bits is a power of two
(e.g. 16, 32, 64) are typically stored *packed*.
For example in a 16-bit audio file, each sample is stored as two bytes,
followed by the two bytes for the next sample and so on.

24-bit data can also be stored packed, so that each sample is stored as three bytes,
followed by the three bytes for the next sample and so on.
This is the most common and also the most space efficient way to store 24-bit data.

However, the 3-byte alignment of packed 24-bit samples can be inconvenient.
In some cases it is easier and/or more efficient to insert an extra byte
so that each sample takes up 4 bytes,
and some hardware devices even require this for 24-bit data.
The extra byte is called *padding*.
If the padding is placed in the least significant byte,
the data is then in the three most significant bytes,
called *left-justified*.
The opposite, where the padding byte is placed in the most significant byte is called
*right-justified*.

## Integer formats
CamillaDSP supports a wide range of *signed* integer formats.
Signed means that they can hold both positive and negative values.
While audio data also can be stored as *unsigned* integers,
this is rarely used in practice.

### S16_LE
Signed 16-bit integers, stored as two bytes per sample, in little-endian byte order.

### S24_3_LE
Signed 24-bit integer, stored *packed* as three bytes in little-endian byte order.
This is the most common 24-bit format.

### S24_4_LJ_LE
Signed 24-bit integer, stored *padded* as four bytes, left justified, in little-endian byte order.
The three most significant bytes hold the audio data,
and the least significant byte is unused padding.
This format is used by some devices in Wasapi exclusive mode,
and may also be stored in .wav files.

### S24_4_RJ_LE
Signed 24-bit integer, stored *padded* as four bytes, right justified, in little-endian byte order.
The three least significant bytes hold the audio data,
and the most significant byte is unused padding.
This format is used by a very small number of devices in the ALSA api,
where it has the somewhat misleading name *S24_LE*.

### S32_LE
Signed 32-bit int, stored as four bytes in little-endian byte order.


## Floating point formats
Floating point formats are often used when exchanging data between an application
and an audio API such as CoreAudio, Wasapi (in shared mode) or PulseAudio.
The convention is that audio data is scaled the value range of -1.0 to +1.0.
Very few audio devices, if any, support these formats in hardware.

### F32_LE
32-bit floating point, stored as four bytes.
Also called *single precision*.
Note that up to 24-bit integers can be converted to and from 32-bit floating point
without loss of precision.

### F64_LE
64-bit floating point, stored as eight bytes in little-endian byte order.
Also called *double precision*.

