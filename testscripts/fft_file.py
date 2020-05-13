# run example: python fft_file.py result_i32.raw S32LE 44100 2

import numpy as np
import numpy.fft as fft
import csv
import yaml
import sys
from matplotlib import pyplot as plt
from matplotlib.patches import Rectangle
import math

fname = sys.argv[1]
datafmt = sys.argv[2]
srate = int(sys.argv[3])
nchannels = int(sys.argv[4])
try:
    window = int(sys.argv[5])
except:
    window = 0

if datafmt == "text":
    with open(fname) as f:
        values = [float(row[0]) for row in csv.reader(f)]
elif datafmt == "FLOAT64LE":
    values = np.fromfile(fname, dtype=float)
elif datafmt == "FLOAT32LE":
    values = np.fromfile(fname, dtype=np.float32)
elif datafmt == "S16LE":
    values = np.fromfile(fname, dtype=np.int16)/(2**15-1)
elif datafmt == "S24LE":
    values = np.fromfile(fname, dtype=np.int32)/(2**23-1)
elif datafmt == "S32LE":
    values = np.fromfile(fname, dtype=np.int32)/(2**31-1)
elif datafmt == "S64LE":
    values = np.fromfile(fname, dtype=np.int64)/(2**31-1)

all_values = np.reshape(values, (nchannels, -1), order='F')

plt.figure(num="FFT of {}".format(fname))
for chan in range(nchannels):
    chanvals = all_values[chan,:]
    npoints = len(chanvals)
    if window>0:
        #chanvals = chanvals[1024:700000]
        npoints = len(chanvals)
        for n in range(window):
            chanvals = chanvals*np.blackman(npoints)
    print(npoints)
    t = np.linspace(0, npoints/srate, npoints, endpoint=False) 
    f = np.linspace(0, srate/2.0, math.floor(npoints/2))
    valfft = fft.fft(chanvals)
    cut = valfft[0:math.floor(npoints/2)]
    gain = 20*np.log10(np.abs(cut))
    if window:
        gain = gain-np.max(gain)
    phase = 180/np.pi*np.angle(cut)
    plt.subplot(2,1,1)
    plt.semilogx(f, gain)
    #plt.subplot(3,1,2)
    #plt.semilogx(f, phase)

    #plt.gca().set(xlim=(10, srate/2.0))
    plt.subplot(2,1,2)
    plt.plot(t, chanvals)


plt.show()

