import csv
from matplotlib import pyplot as plt
import matplotlib.ticker as ticker
import sys
import math
from scipy import interpolate
import numpy

from camilladsp_plot.filters import Biquad


fname = "rme_datasets.csv"
data = []
with open(fname) as f:
    reader = csv.reader(f)
    headers = next(reader)
    for header in headers:
        data.append([])
    _temp = next(reader)
    for line in reader:
        for n, val in enumerate(line):
            if val:
                data[n].append(float(val))


#print(data)
x_5 = data[0]
y_5 = data[1]
x_10 = data[2]
y_10 = data[3]
x_15 = data[4]
y_15 = data[5]
x_20 = data[6]
y_20 = data[7]


#f5 = interpolate.interp1d(x_5, y_5, kind='cubic', bounds_error=False)
#f10 = interpolate.interp1d(x_10, y_10, kind='cubic', bounds_error=False)
#f15 = interpolate.interp1d(x_15, y_15, kind='cubic', bounds_error=False)
#f20 = interpolate.interp1d(x_20, y_20, kind='cubic', bounds_error=False)
#
#
xi = numpy.linspace(20, 20000, 5000)
#yi100 = f100(xi)
#yi80 = f80(xi)
#yi60 = f60(xi)
#yi40 = f40(xi)
#yi20 = f20(xi)
#yi0 = f0(xi)

fig1 = plt.figure(1, figsize=(10,7))
#plt.semilogx(x_100,y_100, linestyle='dashed', marker=".")
#plt.semilogx(x_80,y_80, linestyle='dashed', marker=".")
plt.semilogx(x_5,y_5)
plt.semilogx(x_10,y_10)
plt.semilogx(x_15,y_15)
plt.semilogx(x_20,y_20)

#plt.xlabel("Length")
#plt.ylabel("Time, us")


def make_corr(gain):
    rel_gain = -gain/20
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

gain5 = make_corr(-5)
gain10 = make_corr(-10)
gain15 = make_corr(-15)
gain20 = make_corr(-20)



fig2 = plt.figure(2, figsize=(10,7))
plt.semilogx(x_5,y_5)
plt.semilogx(x_10,y_10)
plt.semilogx(x_15,y_15)
plt.semilogx(x_20,y_20)
plt.semilogx(xi,gain5)
plt.semilogx(xi,gain10)
plt.semilogx(xi,gain15)
plt.semilogx(xi,gain20)




plt.show()
