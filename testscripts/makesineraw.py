# Make a simple spike for testing purposes
import numpy as np
import sys
f = float(sys.argv[2])
fs = float(sys.argv[1])
t = np.linspace(0, 1, num=44100, endpoint=False)
wave = 0.5*np.sin(f*2*np.pi*t)
wave= np.reshape(wave,(-1,1))
wave = np.concatenate((wave, wave), axis=1)

wave64 = wave.astype('float64')

name = "sine_{:.0f}_{:.0f}_f64_2ch.raw".format(f, fs)
#print(wave64)
wave64.tofile(name)


