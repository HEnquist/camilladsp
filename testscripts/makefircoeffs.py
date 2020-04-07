# Make short FIR coeffs in different formats, for testing importing 
import numpy as np

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


float64 = np.array([-1.0, -0.5, 0.0, 0.5, 1.0], dtype="float64")
float32 = np.array([-1.0, -0.5, 0.0, 0.5, 1.0], dtype="float32")
int16 = np.array([-2**15, -2**14, 0.0, 2**14, 2**15-1], dtype="int16")
int24 = np.array([-2**23, -2**22, 0.0, 2**22, 2**23-1], dtype="int32")
int32 = np.array([-2**31, -2**30, 0.0, 2**30, 2**31-1], dtype="int32")

float64.tofile("testdata/float64.raw")
float32.tofile("testdata/float32.raw")
int16.tofile("testdata/int16.raw")
int24.tofile("testdata/int24.raw")
int32.tofile("testdata/int32.raw")
