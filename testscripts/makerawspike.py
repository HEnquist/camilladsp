# Make a simple spike for testing purposes
import numpy as np


spike = np.zeros(2**12, dtype="float64")
spike[1024] = 1.0

spike.tofile("spike_f64.raw")


