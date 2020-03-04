# show_config.py

import numpy as np
import numpy.fft as fft
import csv
import yaml
import sys
from matplotlib import pyplot as plt
from matplotlib.patches import Rectangle
import math

class Conv(object):
    def __init__(self, conf, fs):
        fname = conf['filename']
        with open(fname) as f:
            values = [float(row[0]) for row in csv.reader(f)]
        self.impulse = values
        self.fs = fs


    def gain_and_phase(self, npoints):
        impulselen = len(self.impulse)
        impulse = np.zeros(npoints*2)
        impulse[0:impulselen] = self.impulse
        impfft = fft.fft(impulse)
        cut = impfft[0:npoints]
        f = np.linspace(0, self.fs/2.0, npoints)
        gain = 20*np.log10(np.abs(cut))
        phase = 180/np.pi*np.angle(cut)
        return f, gain, phase

class Biquad(object):
    def __init__(self, conf, fs):
        ftype = conf['type']
        self.s1 = 0
        self.s2 = 0
        if ftype == "Free":
            a0 = 1.0
            a1 = conf['a1']
            a2 = conf['a1']
            b0 = conf['b0']
            b1 = conf['b1']
            b2 = conf['b2']
        if ftype == "Highpass":
            freq = conf['freq']
            q = conf['q']
            omega = 2.0 * np.pi * freq / fs
            sn = np.sin(omega)
            cs = np.cos(omega)
            alpha = sn / (2.0 * q)
            b0 = (1.0 + cs) / 2.0
            b1 = -(1.0 + cs)
            b2 = (1.0 + cs) / 2.0
            a0 = 1.0 + alpha
            a1 = -2.0 * cs
            a2 = 1.0 - alpha
        elif ftype == "Lowpass":
            freq = conf['freq']
            q = conf['q']
            omega = 2.0 * np.pi * freq / fs
            sn = np.sin(omega)
            cs = np.cos(omega)
            alpha = sn / (2.0 * q)
            b0 = (1.0 - cs) / 2.0
            b1 = 1.0 - cs
            b2 = (1.0 - cs) / 2.0
            a0 = 1.0 + alpha
            a1 = -2.0 * cs
            a2 = 1.0 - alpha
        elif ftype == "Peaking":
            freq = conf['freq']
            q = conf['q']
            gain = conf['gain']
            omega = 2.0 * np.pi * freq / fs
            sn = np.sin(omega)
            cs = np.cos(omega)
            ampl = 10.0**(gain / 40.0)
            alpha = sn / (2.0 * q)
            b0 = 1.0 + (alpha * ampl)
            b1 = -2.0 * cs
            b2 = 1.0 - (alpha * ampl)
            a0 = 1.0 + (alpha / ampl)
            a1 = -2.0 * cs
            a2 = 1.0 - (alpha / ampl)
        elif ftype == "Highshelf":
            freq = conf['freq']
            slope = conf['slope']
            gain = conf['gain']
            omega = 2.0 * np.pi * freq / fs
            ampl = 10.0**(gain / 40.0)
            sn = np.sin(omega)
            cs = np.cos(omega)
            alpha = sn / 2.0 * np.sqrt((ampl + 1.0 / ampl) * (1.0 / (slope/12.0) - 1.0) + 2.0)
            beta = 2.0 * np.sqrt(ampl) * alpha
            b0 = ampl * ((ampl + 1.0) + (ampl - 1.0) * cs + beta)
            b1 = -2.0 * ampl * ((ampl - 1.0) + (ampl + 1.0) * cs)
            b2 = ampl * ((ampl + 1.0) + (ampl - 1.0) * cs - beta)
            a0 = (ampl + 1.0) - (ampl - 1.0) * cs + beta
            a1 = 2.0 * ((ampl - 1.0) - (ampl + 1.0) * cs)
            a2 = (ampl + 1.0) - (ampl - 1.0) * cs - beta
        elif ftype == "Lowshelf":
            freq = conf['freq']
            slope = conf['slope']
            gain = conf['gain']
            omega = 2.0 * np.pi * freq / fs
            ampl = 10.0**(gain / 40.0)
            sn = np.sin(omega)
            cs = np.cos(omega)
            alpha = sn / 2.0 * np.sqrt((ampl + 1.0 / ampl) * (1.0 / (slope/12.0) - 1.0) + 2.0)
            beta = 2.0 * np.sqrt(ampl) * alpha
            b0 = ampl * ((ampl + 1.0) - (ampl - 1.0) * cs + beta)
            b1 = 2.0 * ampl * ((ampl - 1.0) - (ampl + 1.0) * cs)
            b2 = ampl * ((ampl + 1.0) - (ampl - 1.0) * cs - beta)
            a0 = (ampl + 1.0) + (ampl - 1.0) * cs + beta
            a1 = -2.0 * ((ampl - 1.0) + (ampl + 1.0) * cs)
            a2 = (ampl + 1.0) + (ampl - 1.0) * cs - beta
        elif ftype == "LowpassFO":
            freq = conf['freq']
            omega = 2.0 * np.pi * freq / fs
            k = np.tan(omega/2.0)
            alpha = 1 + k
            a0 = 1.0
            a1 = -((1 - k)/alpha)
            a2 = 0.0
            b0 = k/alpha
            b1 = k/alpha
            b2 = 0
        elif ftype == "HighpassFO":
            freq = conf['freq']
            omega = 2.0 * np.pi * freq / fs
            k = np.tan(omega/2.0)
            alpha = 1 + k
            a0 = 1.0
            a1 = -((1 - k)/alpha)
            a2 = 0.0
            b0 = 1.0/alpha
            b1 = -1.0/alpha
            b2 = 0
        elif ftype == "Notch":
            freq = conf['freq']
            q = conf['q']
            omega = 2.0 * np.pi * freq / fs
            sn = np.sin(omega)
            cs = np.cos(omega)
            alpha = sn / (2.0 * q)
            b0 = 1.0
            b1 = -2.0 * cs
            b2 = 1.0
            a0 = 1.0 + alpha
            a1 = -2.0 * cs
            a2 = 1.0 - alpha
        elif ftype == "Bandpass":
            freq = conf['freq']
            q = conf['q']
            omega = 2.0 * np.pi * freq / fs
            sn = np.sin(omega)
            cs = np.cos(omega)
            alpha = sn / (2.0 * q)
            b0 = alpha
            b1 = 0.0
            b2 = -alpha
            a0 = 1.0 + alpha
            a1 = -2.0 * cs
            a2 = 1.0 - alpha
        elif ftype == "Allpass":
            freq = conf['freq']
            q = conf['q']
            omega = 2.0 * np.pi * freq / fs
            sn = np.sin(omega)
            cs = np.cos(omega)
            alpha = sn / (2.0 * q)
            b0 = 1.0 - alpha
            b1 = -2.0 * cs
            b2 = 1.0 + alpha
            a0 = 1.0 + alpha
            a1 = -2.0 * cs
            a2 = 1.0 - alpha
        elif ftype == "LinkwitzTransform":
            f0 = conf['freq_act']
            q0 = conf['q_act']
            qt = conf['q_target']
            ft = conf['freq_target']

            d0i = (2.0 * np.pi * f0)**2
            d1i = (2.0 * np.pi * f0)/q0
            c0i = (2.0 * np.pi * ft)**2
            c1i = (2.0 * np.pi * ft)/qt
            fc = (ft+f0)/2.0

            gn = 2 * np.pi * fc/math.tan(np.pi*fc/fs)
            cci = c0i + gn * c1i + gn**2

            b0 = (d0i+gn*d1i + gn**2)/cci 
            b1 = 2*(d0i-gn**2)/cci
            b2 = (d0i - gn*d1i + gn**2)/cci
            a0 = 1.0
            a1 = 2.0 * (c0i-gn**2)/cci
            a2 = ((c0i-gn*c1i + gn**2)/cci)


        self.fs = fs
        self.a1 = a1 / a0
        self.a2 = a2 / a0
        self.b0 = b0 / a0
        self.b1 = b1 / a0
        self.b2 = b2 / a0
        

    def gain_and_phase(self, f):
        z = np.exp(1j*2*np.pi*f/self.fs);
        A = (self.b0 + self.b1*z**(-1) + self.b2*z**(-2))/(1.0 + self.a1*z**(-1) + self.a2*z**(-2))
        gain = 20*np.log10(np.abs(A))
        phase = 180/np.pi*np.angle(A)
        return gain, phase

    def is_stable(self):
        return abs(self.a2)<1.0 and abs(self.a1) < (self.a2+1.0)

    def process_single(self, input):
        out = self.s1 + self.b0 * input
        self.s1 = self.s2 + self.b1 * input - self.a1 * out
        self.s2 = self.b2 * input - self.a2 * out
        return out
        
def dither(bits, wave_in):
    # http://digitalsoundandmusic.com/5-3-7-the-mathematics-of-dithering-and-noise-shaping/
    #     b_orig, the original bit depth
    # b_new, the new bit depth to which samples are to be quantized
    # F_in, an array of N digital audio samples that are to be
    # quantized, dithered, and noise shaped.  It’s assumed that these are read
    # in from a RAW file and are values between
    # –2^b_orig-1 and (2^b_orig-1)-1.
    # c, a scaling factor for the noise shaping
    #Output
    # F_out, an array of N digital audio samples quantized to bit
    # depth b_new using dither and noise shaping*/

    s = (2**(bits-1))
    print(s, bits)
    c = 0.8  # //Other scaling factors can be tried.*/
    e = 0
    wave_out = np.zeros(len(wave_in))
    wave_out_quant = np.zeros(len(wave_in))

    rand_nbrs = np.random.triangular(-1, 0, 1, len(wave_in))

    for i in range(len(wave_in)):
        d = rand_nbrs[i]
        scaled  = wave_in[i] * s
        scaled_plus_dith_and_error = scaled + d + c*e
        wave_out[i] =  round(scaled_plus_dith_and_error)
        wave_out_quant[i] = round(scaled)
        e = scaled - wave_out[i]
    return wave_out/s, wave_out_quant/s

def main():

    fs = 44000
    fignbr = 1
    t = np.linspace(0, 10, 10*fs, endpoint=False)

    wave = np.sin(2*np.pi*1000*t)
    wave_ft = fft.fft(wave)
    cut = wave_ft[0:round(len(wave)/2)]
    f = np.linspace(0, fs/2.0, round(len(wave)/2))
    magn = 20*np.log10(np.abs(cut))

    # plt.figure(1)
    # plt.semilogx(f, magn)
    # plt.title("orig")

    wave_dith, wave_quant = dither(16, wave)

    plt.figure(1)
    plt.plot(t[0:100], wave_dith[0:100]-wave[0:100], t[0:100], wave_quant[0:100]-wave[0:100])
    plt.figure(2)
    plt.plot(t[0:100], wave[0:100], t[0:100], wave_quant[0:100], t[0:100], wave_dith[0:100])

    wave_dith_ft = fft.fft(wave_dith)
    dith_cut = wave_dith_ft[0:round(len(wave)/2)]
    magn_dith = 20*np.log10(np.abs(dith_cut))

    plt.figure(10)
    plt.plot(f, magn_dith)
    plt.ylim((-50, 120)) 
    plt.title("dithered")

    #plt.figure(11)
    #plt.plot(f, magn)
    #plt.ylim((-100, 120)) 
    #plt.title("original")

    wave_quant_ft = fft.fft(wave_quant)
    quant_cut = wave_quant_ft[0:round(len(wave)/2)]
    magn_quant = 20*np.log10(np.abs(quant_cut)+1e-6)

    plt.figure(12)
    plt.plot(f, magn_quant)
    plt.ylim((-50, 120)) 
    plt.title("quantized")
    
    plt.show()

if __name__ == "__main__":
    main()