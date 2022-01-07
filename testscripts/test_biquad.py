class BiquadFilter():
    def __init__(self, a1, a2, b0, b1, b2):
        self.a1 = a1
        self.a2 = a2
        self.b0 = b0
        self.b1 = b1
        self.b2 = b2
        self.s1 = 0.0
        self.s2 = 0.0


    def process_single(self, value):
        out = self.s1 + self.b0 * value
        self.s1 = self.s2 + self.b1 * value - self.a1 * out
        self.s2 = self.b2 * value - self.a2 * out
        return out


from matplotlib import pyplot as plt
values = [0.0, 0.0, -0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
a1 = 0.3
a2 = 0
b0 = 0.3
b1 = 1
b2 = 0
filt = BiquadFilter(a1, a2, b0, b1, b2)
processed = [filt.process_single(val) for val in values]

plt.figure()
plt.plot(values)
plt.plot(processed)
plt.show()

print(values)
print(processed)