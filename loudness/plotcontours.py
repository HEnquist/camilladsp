import csv
from matplotlib import pyplot as plt
import matplotlib.ticker as ticker
import sys
import math
from scipy import interpolate
import numpy

from camilladsp_plot.filters import Biquad


fname = "contours.csv"
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
x_100 = data[0]
y_100 = data[1]
x_80 = data[2]
y_80 = data[3]
x_60 = data[4]
y_60 = data[5]
x_40 = data[6]
y_40 = data[7]
x_20 = data[8]
y_20 = data[9]
x_0 = data[10]
y_0 = data[11]

f100 = interpolate.interp1d(x_100, y_100, kind='cubic', bounds_error=False)
f80 = interpolate.interp1d(x_80, y_80, kind='cubic', bounds_error=False)
f60 = interpolate.interp1d(x_60, y_60, kind='cubic', bounds_error=False)
f40 = interpolate.interp1d(x_40, y_40, kind='cubic', bounds_error=False)
f20 = interpolate.interp1d(x_20, y_20, kind='cubic', bounds_error=False)
f0 = interpolate.interp1d(x_0, y_0, kind='cubic', bounds_error=False)

xi = numpy.linspace(20, 20000, 500)
yi100 = f100(xi)
yi80 = f80(xi)
yi60 = f60(xi)
yi40 = f40(xi)
yi20 = f20(xi)
yi0 = f0(xi)

fig1 = plt.figure(1, figsize=(10,7))
#plt.semilogx(x_100,y_100, linestyle='dashed', marker=".")
#plt.semilogx(x_80,y_80, linestyle='dashed', marker=".")
plt.semilogx(xi,yi100)
plt.semilogx(xi,yi80)
plt.semilogx(xi,yi60)
plt.semilogx(xi,yi40)
plt.semilogx(xi,yi20)
plt.semilogx(xi,yi0)
#plt.xlabel("Length")
#plt.ylabel("Time, us")


diff80 = yi80-yi100
diff60 = yi60-yi100
diff40 = yi40-yi100
diff20 = yi20-yi100
diff0 = yi0-yi100


def make_corr(att):

    conf0 ={"type": "Peaking", "freq": 100, "q":0.1, "gain":att/100+att**3/30000}
    peak0 = Biquad(conf0, 96000)
    _, gain0, _ = peak0.gain_and_phase(xi)
    conf1 ={"type": "Peaking", "freq": 600, "q":0.022, "gain":5*att/6-att**3/30000}
    peak1 = Biquad(conf1, 96000)
    _, gain1, _ = peak1.gain_and_phase(xi)

    conf2 ={"type": "Peaking", "freq": 7000, "q":0.07, "gain":att/10+att**3/50000}
    peak2 = Biquad(conf2, 96000)
    _, gain2, _ = peak2.gain_and_phase(xi)

    conf3 ={"type": "Peaking", "freq": 20000, "q":0.1, "gain":att/2+att**3/500000}
    peak3 = Biquad(conf3, 96000)
    _, gain3, _ = peak3.gain_and_phase(xi)

    gain=numpy.array(gain0)+numpy.array(gain1)+numpy.array(gain2)+numpy.array(gain3)
    return gain

gain20 = make_corr(-20)
gain40 = make_corr(-40)
gain60 = make_corr(-60)
gain80 = make_corr(-80)
gain100 = make_corr(-100)


fig2 = plt.figure(2, figsize=(10,7))
plt.semilogx(xi,diff80)
plt.semilogx(xi,diff60)
plt.semilogx(xi,diff40)
plt.semilogx(xi,diff20)
plt.semilogx(xi,diff0)
plt.semilogx(xi,gain20)
plt.semilogx(xi,gain40)
plt.semilogx(xi,gain60)
plt.semilogx(xi,gain80)
plt.semilogx(xi,gain100)




plt.show()
