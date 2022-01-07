import csv
from matplotlib import pyplot as plt
import matplotlib.ticker as ticker
import sys
import math
import numpy

from camilladsp_plot.filters import Biquad

xi = numpy.linspace(10, 20000, 5000)

def make_corr(gain, ref):
    rel_gain = (ref-gain)/20
    if rel_gain < 0:
        rel_gain = 0.0
    elif rel_gain > 1:
        rel_gain = 1.0
    high_gain = 10*rel_gain
    low_gain = 10*rel_gain

    conf0 ={"type": "Lowshelf", "freq": 70, "slope":12, "gain": low_gain}
    low = Biquad(conf0, 96000)
    _, lowgain, _ = low.gain_and_phase(xi)
    conf1 ={"type": "Highshelf", "freq": 3500, "slope":12, "gain": high_gain}
    high = Biquad(conf1, 96000)
    _, highgain, _ = high.gain_and_phase(xi)
    gain=gain + numpy.array(highgain)+numpy.array(lowgain)
    return gain

fig2 = plt.figure(2, figsize=(8,5))
gains = [0, -5, -10, -15, -20, -25, -30]
legend = list(gains)
legend[1] = "-5 (ref.)"
for gain in gains:
    y = make_corr(gain, -5)
    plt.semilogx(xi,y)
plt.grid()
plt.xlabel("Frequency, Hz")
plt.ylabel("Gain, dB")
plt.legend(legend)
plt.title("reference_level = -5dB, high_boost = 10 dB, low_boost = 10 dB")

plt.show()
