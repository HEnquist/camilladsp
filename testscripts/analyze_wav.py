import os
import struct
import logging

sampleformats = {1: "int",
    3: "float",
    }

def analyze_chunk(type, start, length, file, wav_info):
    if type == "fmt ":
        data = file.read(length)
        wav_info['SampleFormat'] = sampleformats[struct.unpack('<H', data[0:2])[0]]
        wav_info['NumChannels'] = struct.unpack('<H', data[2:4])[0]
        wav_info['SampleRate'] = struct.unpack('<L', data[4:8])[0]
        wav_info['ByteRate'] = struct.unpack('<L', data[8:12])[0]
        wav_info['BytesPerFrame'] = struct.unpack('<H', data[12:14])[0]
        wav_info['BitsPerSample'] = struct.unpack('<H', data[14:16])[0]
        bytes_per_sample = wav_info['BytesPerFrame']/wav_info['NumChannels']
        if wav_info['SampleFormat'] == "int":
            if wav_info['BitsPerSample'] == 16:
                sfmt = "S16LE"
            elif wav_info['BitsPerSample'] == 24 and bytes_per_sample == 3:
                sfmt = "S24LE3"
            elif wav_info['BitsPerSample'] == 24 and bytes_per_sample == 4:
                sfmt = "S24LE"
            elif wav_info['BitsPerSample'] == 32:
                sfmt = "S32LE"
        elif wav_info['SampleFormat'] == "float":
            if wav_info['BitsPerSample'] == 32:
                sfmt = "FLOAT32LE"
            elif wav_info['BitsPerSample'] == 64:
                sfmt = "FLOAT64LE"
        else:
            sfmt = "unknown"
        wav_info['SampleFormat'] = sfmt
    elif type == "data":
        wav_info['DataStart'] = start
        wav_info['DataLength'] = length
    

def read_wav_header(filename):
    """ 
    Reads the wav header to extract sample format, number of channels, and location of the audio data in the file
    """
    logging.basicConfig(level=logging.DEBUG)
    try:
        file_in = open(filename, 'rb')
    except IOError as err:
        logging.debug("Could not open input file %s" % (strWAVFile))
        return

    # Read fixed header
    buf_header = file_in.read(12)
    # Verify that the correct identifiers are present
    if (buf_header[0:4] != b"RIFF") or \
       (buf_header[8:12] != b"WAVE"): 
         logging.debug("Input file is not a standard WAV file")
         return

    wav_info = {}

    # Get file length
    file_in.seek(0, 2) # Seek to end of file
    input_filesize = file_in.tell()

    next_chunk_location = 12 # skip the fixed header
    while True:
        file_in.seek(next_chunk_location)
        buf_header = file_in.read(8)
        chunk_type = buf_header[0:4].decode("utf-8")
        chunk_length = struct.unpack('<L', buf_header[4:8])[0]
        logging.debug("Found chunk of type {}, length {}".format(chunk_type, chunk_length))
        analyze_chunk(chunk_type, next_chunk_location, chunk_length, file_in, wav_info)
        next_chunk_location += (8 + chunk_length) 
        if next_chunk_location >= input_filesize:
            break
    file_in.close()
    return wav_info
 
if __name__ == "__main__":
    import sys
    info = read_wav_header(sys.argv[1])
    print("Wav properties:")
    for name, val in info.items():
        print("{} : {}".format(name, val))
