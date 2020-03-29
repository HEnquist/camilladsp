import numpy as np


spike = np.zeros(2**16, dtype="int32")
spike[0] = 2**31-1

spike.tofile("spike_i32.raw")


