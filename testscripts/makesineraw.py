# Make simple sines for testing purposes
# Example: 20 seconds of 1kHz + 2 kHz at 44.1 kHz
# > python testscripts/makesineraw.py 44100 20 1000 2000
import numpy as np
import sys
f = float(sys.argv[3])
fs = float(sys.argv[1])
length = int(sys.argv[2])
t = np.linspace(0, 20, num=int(20*fs), endpoint=False)
wave = 0.5*np.sin(f*2*np.pi*t)
f_label = "{:.0f}".format(f)
for f2 in sys.argv[4:]:
    f2f = float(f2)
    wave += 0.5*np.sin(f2f*2*np.pi*t)
    f_label = "{}-{:.0f}".format(f_label, f2f)

wave= np.reshape(wave,(-1,1))
wave = np.concatenate((wave, wave), axis=1)

wave64 = wave.astype('float64')

name = "sine_{}_{:.0f}_{}s_f64_2ch.raw".format(f_label, fs, length)
#print(wave64)
wave64.tofile(name)


