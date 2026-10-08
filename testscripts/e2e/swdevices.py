"""Helpers for the software backend suite: the device blocks, the formats, and the wav header.

These tests run against a stock build, so the only devices available are the ones every
release has: `RawFile`, `WavFile`, `Stdin`, `SignalGenerator`, `File` and `Stdout`. That
is a wider matrix than the dummy suites need, over device type, sample format and the
byte options, so the device blocks are built here rather than checked in as a config per
combination. `file_devices.yml` is the base they are swapped into, the same one
test_file_devices.py uses, and the blocks it starts with are `FILE_CAPTURE` and
`FILE_PLAYBACK` in conftest.

The encoders are the other half. A round trip through a sample format is only assertable
byte for byte if the test writes the input in the same layout CamillaDSP reads, so the
layout is written out here once: what the 24 bit formats put where, and which byte is
padding.
"""

import struct

import numpy as np

# Every format a file or stdio device can carry, which is the whole of BinarySampleFormat.
FORMATS = (
    "S16_LE",
    "S24_3_LE",
    "S24_4_RJ_LE",
    "S24_4_LJ_LE",
    "S32_LE",
    "F32_LE",
    "F64_LE",
)

BYTES_PER_SAMPLE = {
    "S16_LE": 2,
    "S24_3_LE": 3,
    "S24_4_RJ_LE": 4,
    "S24_4_LJ_LE": 4,
    "S32_LE": 4,
    "F32_LE": 4,
    "F64_LE": 8,
}


def encode(fmt, values):
    """Interleaved float samples in [-1, 1) as the bytes a device of `fmt` reads.

    The two 4 byte 24 bit layouts are the ones worth spelling out. `S24_4_RJ_LE` puts the
    three data bytes at the low end and a padding byte at the high end, which CamillaDSP
    zeroes on write and ignores on read, so it is not a sign extended 32 bit value.
    `S24_4_LJ_LE` is the other way round, the data in the top three bytes and the padding
    at the bottom, which is the same as the 24 bit value shifted up by eight.
    """
    values = np.asarray(values, dtype=np.float64).ravel()
    if fmt == "F64_LE":
        return values.astype("<f8").tobytes()
    if fmt == "F32_LE":
        return values.astype("<f4").tobytes()
    if fmt == "S16_LE":
        return np.round(values * 2**15).astype("<i2").tobytes()
    if fmt == "S32_LE":
        # Through int64 first, since the rounded value of a sample close to full scale
        # does not fit in an int32 and numpy would wrap it rather than say so.
        return np.round(values * 2**31).astype("<i8").astype("<i4").tobytes()
    ints = np.round(values * 2**23).astype("<i4")
    if fmt == "S24_3_LE":
        return ints.view("u1").reshape(-1, 4)[:, :3].tobytes()
    if fmt == "S24_4_RJ_LE":
        padded = np.zeros((len(ints), 4), dtype="u1")
        padded[:, :3] = ints.view("u1").reshape(-1, 4)[:, :3]
        return padded.tobytes()
    if fmt == "S24_4_LJ_LE":
        return (ints.astype("<i8") * 256).astype("<i4").tobytes()
    raise ValueError(f"unknown sample format {fmt}")


def decode(fmt, data, channels=2):
    """The inverse of `encode`, as one column per channel.

    Used where the output format differs from the input one and the comparison is a
    tolerance rather than a byte match.
    """
    if fmt == "F64_LE":
        values = np.frombuffer(data, dtype="<f8")
    elif fmt == "F32_LE":
        values = np.frombuffer(data, dtype="<f4").astype(np.float64)
    elif fmt == "S16_LE":
        values = np.frombuffer(data, dtype="<i2").astype(np.float64) / 2**15
    elif fmt == "S32_LE":
        values = np.frombuffer(data, dtype="<i4").astype(np.float64) / 2**31
    elif fmt == "S24_4_LJ_LE":
        values = np.frombuffer(data, dtype="<i4").astype(np.float64) / 2**31
    elif fmt in ("S24_3_LE", "S24_4_RJ_LE"):
        raw = np.frombuffer(data, dtype="u1").reshape(-1, BYTES_PER_SAMPLE[fmt])
        wide = np.zeros((len(raw), 4), dtype="u1")
        # Left justify the three data bytes so the sign bit lands where int32 wants it,
        # then scale the extra eight bits back out.
        wide[:, 1:] = raw[:, :3]
        values = wide.view("<i4").ravel().astype(np.float64) / 2**31
    else:
        raise ValueError(f"unknown sample format {fmt}")
    return values.reshape(-1, channels)


def _options(lines):
    """Render the optional keys of a device block, indented to match."""
    return "".join(f"\n    {line}" for line in lines if line)


def raw_capture(filename, fmt="F64_LE", extra_samples=None, skip_bytes=None, read_bytes=None):
    """A `RawFile` capture block, the one `file_devices.yml` already carries."""
    options = [
        f"extra_samples: {extra_samples}" if extra_samples is not None else None,
        f"skip_bytes: {skip_bytes}" if skip_bytes is not None else None,
        f"read_bytes: {read_bytes}" if read_bytes is not None else None,
    ]
    return (
        f"  capture:\n    type: RawFile\n    channels: 2\n"
        f"    filename: {filename}\n    format: {fmt}" + _options(options)
    )


def wav_capture(filename, extra_samples=None):
    """A `WavFile` capture block, which takes its channels and rate from the header."""
    options = [f"extra_samples: {extra_samples}" if extra_samples is not None else None]
    return f"  capture:\n    type: WavFile\n    filename: {filename}" + _options(options)


def stdin_capture(fmt="F64_LE", channels=2, extra_samples=None):
    options = [f"extra_samples: {extra_samples}" if extra_samples is not None else None]
    return (
        f"  capture:\n    type: Stdin\n    channels: {channels}\n    format: {fmt}"
        + _options(options)
    )


def generator_capture(signal, channels=2):
    """A `SignalGenerator` capture block. `signal` is the inner mapping, as one line."""
    return (
        f"  capture:\n    type: SignalGenerator\n    channels: {channels}\n"
        f"    signal: {{{signal}}}"
    )


def file_playback(filename, fmt="F64_LE", wav_header=None, use_rf64=None):
    options = [
        f"wav_header: {str(wav_header).lower()}" if wav_header is not None else None,
        f"use_rf64: {str(use_rf64).lower()}" if use_rf64 is not None else None,
    ]
    return (
        f"  playback:\n    type: File\n    channels: 2\n"
        f"    filename: {filename}\n    format: {fmt}" + _options(options)
    )


def stdout_playback(fmt="F64_LE", wav_header=None):
    options = [f"wav_header: {str(wav_header).lower()}" if wav_header is not None else None]
    return f"  playback:\n    type: Stdout\n    channels: 2\n    format: {fmt}" + _options(options)


# What goes where `queuelimit` is, for the tests that add a resampler or a second rate.
QUEUELIMIT = "  queuelimit: 4"


def resampling(capture_samplerate, resampler):
    """The devices keys that put a resampler in the path, to replace `QUEUELIMIT`.

    `resampler` is the body of the resampler mapping, without the leading indent of its
    first line, so a caller writes it the way it appears in a config.
    """
    body = "\n".join(f"    {line}" for line in resampler.strip().splitlines())
    return f"{QUEUELIMIT}\n  capture_samplerate: {capture_samplerate}\n  resampler:\n{body}"


def read_exactly(stream, nbytes):
    """Read exactly `nbytes` from a pipe, or raise if it ends first.

    The free running devices produce hundreds of megabytes a second, so a test that wants
    a second of audio out of one has to bound what it takes rather than let it run: a
    `SignalGenerator` never ends by itself, and pointing its playback at a file would
    fill the runner's disk before anything could stop it. Reading a fixed number of bytes
    and then asking the process to exit is what keeps that in hand, and the pipe filling
    up in the meantime is what stops the device running ahead.
    """
    chunks = []
    got = 0
    while got < nbytes:
        chunk = stream.read(nbytes - got)
        if not chunk:
            raise EOFError(f"stdout ended after {got} bytes, wanted {nbytes}")
        chunks.append(chunk)
        got += len(chunk)
    return b"".join(chunks)


def wav_chunks(path):
    """The chunk layout of a wav file: its magic, its size field, and each chunk found.

    Small enough to write out rather than take a dependency for, and the placeholder
    sizes are the point: a streaming wav leaves them at 0xFFFFFFFF, and an RF64 file
    leaves them there deliberately and puts the real lengths in `ds64`.
    """
    with open(path, "rb") as wav:
        data = wav.read()
    magic = data[0:4].decode("latin1")
    riff_size = struct.unpack("<I", data[4:8])[0]
    form = data[8:12].decode("latin1")
    found = {}
    pos = 12
    while pos + 8 <= len(data):
        chunk_id = data[pos : pos + 4].decode("latin1").strip()
        size = struct.unpack("<I", data[pos + 4 : pos + 8])[0]
        found[chunk_id] = (size, pos + 8)
        if size == 0xFFFFFFFF:
            # A placeholder length says nothing about where the next chunk starts, and
            # the only chunk written with one is `data`, which is last anyway.
            break
        pos += 8 + size + (size & 1)
    return {
        "magic": magic,
        "riff_size": riff_size,
        "form": form,
        "chunks": found,
        "total": len(data),
    }
