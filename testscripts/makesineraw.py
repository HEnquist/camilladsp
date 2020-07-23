# Make a simple sine for testing purposes
import numpy as np
import sys
f = float(sys.argv[2])
fs = float(sys.argv[1])
length = int(sys.argv[3])
t = np.linspace(0, 20, num=int(20*fs), endpoint=False)
wave = 0.5*np.sin(f*2*np.pi*t)
wave= np.reshape(wave,(-1,1))
wave = np.concatenate((wave, wave), axis=1)

wave64 = wave.astype('float64')

name = "sine_{:.0f}_{:.0f}_{}s_f64_2ch.raw".format(f, fs, length)
#print(wave64)
wave64.tofile(name)


