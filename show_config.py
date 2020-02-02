# show_config.py

import numpy as np
import numpy.fft as fft
import csv
import yaml
import sys
from matplotlib import pyplot as plt
from matplotlib.patches import Rectangle

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
    if label is not None:
        ax.text((x0+x1)/2, (y0+y1)/2, label, horizontalalignment='right', verticalalignment='bottom')

def draw_box(ax, level, size, label=None):
    x0 = 2*level-0.75
    y0 = -size/2 -0.5
    rect = Rectangle((x0, y0), 1.5, size, linewidth=1,edgecolor='g',facecolor='none', linestyle='--')
    ax.add_patch(rect)
    if label is not None:
        ax.text(2*level, size/2, label, horizontalalignment='center', verticalalignment='bottom')

def main():
    fname = sys.argv[1]

    conffile = open(fname)

    conf = yaml.safe_load(conffile)
    print(conf)

    srate = conf['devices']['samplerate']
    buflen = conf['devices']['buffersize']
    print (srate)

    fvect = np.linspace(0, srate/2.0, 10*buflen)

    fignbr = 1
    for filter, fconf in conf['filters'].items():
        if fconf['type'] == 'Biquad':
            kladd = Biquad(fconf['parameters'], srate)
            plt.figure(fignbr)
            magn, phase = kladd.gain_and_phase(fvect)
            plt.semilogx(fvect, magn)
            plt.title(filter)
            fignbr += 1
        elif fconf['type'] == 'Conv':
            kladd = Conv(fconf['parameters'], srate)
            plt.figure(fignbr)
            ftemp, magn, phase = kladd.gain_and_phase(len(fvect))
            plt.semilogx(fvect, magn)
            plt.title(filter)
            fignbr += 1

    stages = []
    fig = plt.figure(fignbr)
    
    ax = fig.add_subplot(111, aspect='equal')
    # add input
    channels = []
    capture_channels = int(conf['devices']['capture']['channels'])
    for n in range(capture_channels):
        label = "ch {}".format(n) 
        b = Block(label)
        b.place(0, -capture_channels/2 + n)
        b.draw(ax)
        channels.append([b])
    draw_box(ax, 0, capture_channels, label=conf['devices']['capture']['device'])
    stages.append(channels)

    # loop through pipeline

    total_length = 0
    stage_start = 0
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
                b.place(total_length*2, -active_channels/2 + n)
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
                b.place(ch_step*2, -active_channels/2 + ch_nbr)
                b.draw(ax)
                src_p = stages[-1][ch_nbr][-1].output_point()
                dest_p = b.input_point()
                draw_arrow(ax, src_p, dest_p)
                stages[-1][ch_nbr].append(b)


    total_length += 1
    for n in range(active_channels):
        label = "ch {}".format(n) 
        b = Block(label)
        b.place(2*total_length, -active_channels/2 + n)
        b.draw(ax)
        src_p = stages[-1][n][-1].output_point()
        dest_p = b.input_point()
        draw_arrow(ax, src_p, dest_p)
        channels.append([b])
    draw_box(ax, total_length, active_channels, label=conf['devices']['playback']['device'])
    stages.append(channels)
    
    ax.set(xlim=(-1, 2*total_length+1), ylim=(-3, 3))

    plt.show()

if __name__ == "__main__":
    main()