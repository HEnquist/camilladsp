# Make a simple sine for testing purposes
import numpy as np
import sys
import time
import math

f1 = 1000
f2 = 1200
fs = 44100
length = 1024
active = True
period = 5
t_end = 0

while True:
    t_start = t_end
    t_end = t_start + length/fs
    if math.floor(t_start/period)%2 == 0:
        t = np.linspace(t_start, t_end, num=length, endpoint=False)
        wave1 = 0.5*np.sin(f1*2*np.pi*t)
        wave2 = 0.5*np.sin(f2*2*np.pi*t)
        wave1 = np.reshape(wave1,(-1,1))
        wave2 = np.reshape(wave2,(-1,1))
        wave = np.concatenate((wave1, wave2), axis=1)

        wave64 = wave.astype('float64')
        sys.stdout.buffer.write(wave64.tobytes())
    else:
        time.sleep(t_end-t_start)
    


