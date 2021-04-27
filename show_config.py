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
        if not conf:
            conf = {values: [1.0]}
        if 'filename' in conf:
            fname = conf['filename']
            values = []
            if 'format' not in conf:
                conf['format'] = "text"
            if conf['format'] == "text":
                with open(fname) as f:
                    values = [float(row[0]) for row in csv.reader(f)]
            elif conf['format'] == "FLOAT64LE":
                values = np.fromfile(fname, dtype=float)
            elif conf['format'] == "FLOAT32LE":
                values = np.fromfile(fname, dtype=np.float32)
            elif conf['format'] == "S16LE":
                values = np.fromfile(fname, dtype=np.int16)/(2**15-1)
            elif conf['format'] == "S24LE":
                values = np.fromfile(fname, dtype=np.int32)/(2**23-1)
            elif conf['format'] == "S32LE":
                values = np.fromfile(fname, dtype=np.int32)/(2**31-1)
        else:
            values = conf['values']
        self.impulse = values
        self.fs = fs

    def gain_and_phase(self):
        impulselen = len(self.impulse)
        npoints = impulselen
        if npoints < 300:
            npoints = 300
        impulse = np.zeros(npoints*2)
        impulse[0:impulselen] = self.impulse
        impfft = fft.fft(impulse)
        cut = impfft[0:npoints]
        f = np.linspace(0, self.fs/2.0, npoints)
        gain = 20*np.log10(np.abs(cut))
        phase = 180/np.pi*np.angle(cut)
        return f, gain, phase

    def get_impulse(self):
        t = np.linspace(0, len(self.impulse)/self.fs, len(self.impulse), endpoint=False)
        return t, self.impulse

class DiffEq(object):
    def __init__(self, conf, fs):
        self.fs = fs
        self.a = conf['a']
        self.b = conf['b']
        if len(self.a)==0:
            self.a=[1.0]
        if len(self.b)==0:
            self.b=[1.0]

    def gain_and_phase(self, f):
        z = np.exp(1j*2*np.pi*f/self.fs);
        A1=np.zeros(z.shape)
        for n, bn in enumerate(self.b):
            A1 = A1 + bn*z**(-n)
        A2=np.zeros(z.shape)
        for n, an in enumerate(self.a):
            A2 = A2 + an*z**(-n)  
        A = A1/A2  
        gain = 20*np.log10(np.abs(A))
        phase = 180/np.pi*np.angle(A)
        return gain, phase
    
    def is_stable(self):
        # TODO
        return None

class BiquadCombo(object):

    def Butterw_q(self, order):
        odd = order%2 > 0
        n_so = math.floor(order/2.0)
        qvalues = []
        for n in range(0, n_so):
            q = 1/(2.0*math.sin((math.pi/order)*(n + 1/2)))
            qvalues.append(q)
        if odd:
            qvalues.append(-1.0)
        return qvalues

    def __init__(self, conf, fs):
        self.ftype = conf['type']
        self.order = conf['order']
        self.freq = conf['freq']
        self.fs = fs
        if self.ftype == "LinkwitzRileyHighpass":
            #qvalues = self.LRtable[self.order]
            q_temp = self.Butterw_q(self.order/2)
            if (self.order/2)%2 > 0:
                q_temp = q_temp[0:-1]
                qvalues = q_temp + q_temp + [0.5]
            else:
                qvalues = q_temp + q_temp
            type_so = "Highpass"
            type_fo = "HighpassFO"

        elif self.ftype == "LinkwitzRileyLowpass":
            q_temp = self.Butterw_q(self.order/2)
            if (self.order/2)%2 > 0:
                q_temp = q_temp[0:-1]
                qvalues = q_temp + q_temp + [0.5]
            else:
                qvalues = q_temp + q_temp
            type_so = "Lowpass"
            type_fo = "LowpassFO"
        elif self.ftype == "ButterworthHighpass":
            qvalues = self.Butterw_q(self.order)
            type_so = "Highpass"
            type_fo = "HighpassFO"
        elif self.ftype == "ButterworthLowpass":
            qvalues = self.Butterw_q(self.order)
            type_so = "Lowpass"
            type_fo = "LowpassFO"
        self.biquads = []
        print(qvalues)
        for q in qvalues:
            if q >= 0:
                bqconf = {'freq': self.freq, 'q': q, 'type': type_so}
            else:
                bqconf = {'freq': self.freq, 'type': type_fo}
            self.biquads.append(Biquad(bqconf, self.fs))

    def is_stable(self):
        # TODO
        return None

    def gain_and_phase(self, f):
        A = np.ones(f.shape)
        for bq in self.biquads:
            A = A * bq.complex_gain(f)
        gain = 20*np.log10(np.abs(A))
        phase = 180/np.pi*np.angle(A)
        return gain, phase

class Biquad(object):
    def __init__(self, conf, fs):
        ftype = conf['type']
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
        elif ftype == "HighshelfFO":
            freq = conf['freq']
            gain = conf['gain']
            omega = 2.0 * np.pi * freq / fs
            ampl = 10.0**(gain / 40.0)
            tn = np.tan(omega/2)
            b0 = ampl*tn + ampl**2
            b1 = ampl*tn - ampl**2
            b2 = 0.0
            a0 = ampl*tn + 1
            a1 = ampl*tn - 1
            a2 = 0.0
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
        elif ftype == "LowshelfFO":
            freq = conf['freq']
            gain = conf['gain']
            omega = 2.0 * np.pi * freq / fs
            ampl = 10.0**(gain / 40.0)
            tn = np.tan(omega/2)
            b0 = ampl**2*tn + ampl
            b1 = ampl**2*tn - ampl
            b2 = 0.0
            a0 = tn + ampl
            a1 = tn - ampl
            a2 = 0.0
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
        elif ftype == "AllpassFO":
            freq = conf['freq']
            omega = 2.0 * np.pi * freq / fs
            tn = np.tan(omega/2.0)
            alpha = (tn + 1.0)/(tn - 1.0)
            b0 = 1.0
            b1 = alpha
            b2 = 0.0
            a0 = alpha
            a1 = 1.0
            a2 = 0.0
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
        
    def complex_gain(self, f):
        z = np.exp(1j*2*np.pi*f/self.fs);
        A = (self.b0 + self.b1*z**(-1) + self.b2*z**(-2))/(1.0 + self.a1*z**(-1) + self.a2*z**(-2))
        return A

    def gain_and_phase(self, f):
        A = self.complex_gain(f)
        gain = 20*np.log10(np.abs(A))
        phase = 180/np.pi*np.angle(A)
        return gain, phase

    def is_stable(self):
        return abs(self.a2)<1.0 and abs(self.a1) < (self.a2+1.0)
        
class Block(object):
    def __init__(self, label):
        self.label = label
        self.x = None
        self.y = None

    def place(self, x, y):
        self.x = x
        self.y = y

    def draw(self, ax):
        rect = Rectangle((self.x-0.5, self.y-0.25), 1.0, 0.5, linewidth=1,edgecolor='r',facecolor='none')
        ax.add_patch(rect)
        ax.text(self.x, self.y, self.label, horizontalalignment='center', verticalalignment='center')


    def input_point(self):
        return self.x-0.5, self.y

    def output_point(self):
        return self.x+0.5, self.y

def draw_arrow(ax, p0, p1, label=None):
    x0, y0 = p0
    x1, y1 = p1
    ax.arrow(x0, y0, x1-x0, y1-y0, width=0.01, length_includes_head=True, head_width=0.1)
    if y1 > y0:
        hal = 'right'
        val = 'bottom'
    else:
        hal = 'right'
        val = 'top'
    if label is not None:
        ax.text(x0+(x1-x0)*2/3, y0+(y1-y0)*2/3, label, horizontalalignment=hal, verticalalignment=val)

def draw_box(ax, level, size, label=None):
    x0 = 2*level-0.75
    y0 = -size/2
    rect = Rectangle((x0, y0), 1.5, size, linewidth=1,edgecolor='g',facecolor='none', linestyle='--')
    ax.add_patch(rect)
    if label is not None:
        ax.text(2*level, size/2, label, horizontalalignment='center', verticalalignment='bottom')

def main():
    print('This script is deprecated. Please use the "plotcamillaconf" tool\nfrom the pycamilladsp-plot library instead.')
    fname = sys.argv[1]

    conffile = open(fname)

    conf = yaml.safe_load(conffile)
    print(conf)

    srate = conf['devices']['samplerate']
    #if "chunksize" in conf['devices']:
    #    buflen = conf['devices']['chunksize']
    #else:
    #    buflen = conf['devices']['buffersize']
    #print (srate)
    fignbr = 1

    if 'filters' in conf:
        fvect = np.linspace(1, (srate*0.95)/2.0, 10000)
        for filter, fconf in conf['filters'].items():
            if fconf['type'] in ('Biquad', 'DiffEq', 'BiquadCombo'):
                if fconf['type'] == 'DiffEq':
                    kladd = DiffEq(fconf['parameters'], srate)
                elif fconf['type'] == 'BiquadCombo':
                    kladd = BiquadCombo(fconf['parameters'], srate)
                else:
                    kladd = Biquad(fconf['parameters'], srate)
                plt.figure(num=filter)
                magn, phase = kladd.gain_and_phase(fvect)
                stable = kladd.is_stable()
                plt.subplot(2,1,1)
                plt.semilogx(fvect, magn)
                plt.title("{}, stable: {}\nMagnitude".format(filter, stable))
                plt.subplot(2,1,2)
                plt.semilogx(fvect, phase)
                plt.title("Phase")
                fignbr += 1
            elif fconf['type'] == 'Conv':
                if 'parameters' in fconf:
                    kladd = Conv(fconf['parameters'], srate)
                else:
                    kladd = Conv(None, srate)
                plt.figure(num=filter)
                ftemp, magn, phase = kladd.gain_and_phase()
                plt.subplot(2,1,1)
                plt.semilogx(ftemp, magn)
                plt.title("FFT of {}".format(filter))
                plt.gca().set(xlim=(10, srate/2.0))
                #fignbr += 1
                #plt.figure(fignbr)
                t, imp = kladd.get_impulse()
                plt.subplot(2,1,2)
                plt.plot(t, imp)
                plt.title("Impulse response of {}".format(filter))
                fignbr += 1

    stages = []
    fig = plt.figure(fignbr)
    
    ax = fig.add_subplot(111, aspect='equal')
    # add input
    channels = []
    active_channels = int(conf['devices']['capture']['channels'])
    for n in range(active_channels):
        label = "ch {}".format(n) 
        b = Block(label)
        b.place(0, -active_channels/2 + 0.5 + n)
        b.draw(ax)
        channels.append([b])
    if 'device' in conf['devices']['capture']:
        capturename = conf['devices']['capture']['device']
    else:
        capturename = conf['devices']['capture']['filename']
    draw_box(ax, 0, active_channels, label=capturename)
    stages.append(channels)

    # loop through pipeline

    total_length = 0
    stage_start = 0
    if 'pipeline' in conf:
        for step in conf['pipeline']:
            stage = len(stages)
            if step['type'] == 'Mixer':
                total_length += 1
                name = step['name']
                mixconf = conf['mixers'][name]
                active_channels = int(mixconf['channels']['out'])
                channels = [[]]*active_channels
                for n in range(active_channels):
                    label = "ch {}".format(n)
                    b = Block(label)
                    b.place(total_length*2, -active_channels/2 + 0.5 + n)
                    b.draw(ax)
                    channels[n] = [b]
                for mapping in mixconf['mapping']:
                    dest_ch = int(mapping['dest'])
                    for src in mapping['sources']:
                        src_ch = int(src['channel'])
                        label = "{} dB".format(src['gain'])
                        if src['inverted'] == 'False':
                            label = label + '\ninv.'
                        src_p = stages[-1][src_ch][-1].output_point()
                        dest_p = channels[dest_ch][0].input_point()
                        draw_arrow(ax, src_p, dest_p, label=label)
                draw_box(ax, total_length, active_channels, label=name)
                stages.append(channels)
                stage_start = total_length
            elif step['type'] == 'Filter':
                ch_nbr = step['channel']
                for name in step['names']:
                    b = Block(name)
                    ch_step = stage_start + len(stages[-1][ch_nbr])
                    total_length = max((total_length, ch_step))
                    b.place(ch_step*2, -active_channels/2 + 0.5 + ch_nbr)
                    b.draw(ax)
                    src_p = stages[-1][ch_nbr][-1].output_point()
                    dest_p = b.input_point()
                    draw_arrow(ax, src_p, dest_p)
                    stages[-1][ch_nbr].append(b)


    total_length += 1
    channels = []
    for n in range(active_channels):
        label = "ch {}".format(n) 
        b = Block(label)
        b.place(2*total_length, -active_channels/2 + 0.5 + n)
        b.draw(ax)
        src_p = stages[-1][n][-1].output_point()
        dest_p = b.input_point()
        draw_arrow(ax, src_p, dest_p)
        channels.append([b])
    if 'device' in conf['devices']['playback']:
        playname = conf['devices']['playback']['device']
    else:
        playname = conf['devices']['playback']['filename']
    draw_box(ax, total_length, active_channels, label=playname)
    stages.append(channels)
    
    nbr_chan = [len(s) for s in stages]
    ylim = math.ceil(max(nbr_chan)/2.0) + 0.5
    ax.set(xlim=(-1, 2*total_length+1), ylim=(-ylim, ylim))
    plt.axis('off')
    plt.show()

if __name__ == "__main__":
    main()