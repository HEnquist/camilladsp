import sys
import time



out = open("/dev/stdout", "wb")
data = bytearray(8*4800)
for n in range(5):
    for m in range(100):
        print("send data", file=sys.stderr)
        out.write(data)
        time.sleep(0.1)
    print("sleep", file=sys.stderr)
    time.sleep(10)
