import numpy as np

# float64
start = np.linspace(0, 1, 16)
mid  = np.linspace(1,-1, 32)
end = np.linspace(-1, 0, 16)
impulse = np.concatenate((start,mid,end))
float64 = np.array(impulse, dtype="float64")
float32 = np.array(impulse, dtype="float32")
int16 = np.array((2**15-1)*impulse, dtype="int16")
int24 = np.array((2**23-1)*impulse, dtype="int32")
int32 = np.array((2**31-1)*impulse, dtype="int32")

float64.tofile("float64.raw")
float32.tofile("float32.raw")
int16.tofile("int16.raw")
int24.tofile("int24.raw")
int32.tofile("int32.raw")
