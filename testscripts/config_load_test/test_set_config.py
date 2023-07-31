import time
import camilladsp

cdsp = camilladsp.CamillaClient("localhost", 1234)
cdsp.connect()

# Read the configs
configs = []
for n in range(4):
    with open(f"conf{n+1}.yml") as f:
        configs.append(f.read())

# Apply them all slowly
print("Changing slowly")
for n in range(4):
    print(f"Set conf{n+1}")
    cdsp.config.set_active_raw(configs[n])
    time.sleep(2)
    conf = cdsp.config.active()
    print("active:", conf["filters"]["testfilter"]["description"])
    assert conf["filters"]["testfilter"]["description"] == f"nbr {n+1}"


# Apply them with short delay
print("Changing with short delay")
print("Set conf1")
cdsp.config.set_active_raw(configs[0])
time.sleep(1)
conf = cdsp.config.active()
print("active:", conf["filters"]["testfilter"]["description"])
assert conf["filters"]["testfilter"]["description"] == "nbr 1"
print("Set conf2, 3, 4, 2, 3, 4, ...")
for _ in range(100):
    print(".", end="", flush=True)
    cdsp.config.set_active_raw(configs[1])
    time.sleep(0.1)
    cdsp.config.set_active_raw(configs[2])
    time.sleep(0.1)
    cdsp.config.set_active_raw(configs[3])
    time.sleep(0.5)
    conf = cdsp.config.active()
    desc = conf["filters"]["testfilter"]["description"]
    assert conf["filters"]["testfilter"]["description"] == "nbr 4", f"{desc} != nbr 4"

# Apply them with very short delay
print("Changing fast")
print("Set conf1")
cdsp.config.set_active_raw(configs[0])
time.sleep(1)
conf = cdsp.config.active()
print("active:", conf["filters"]["testfilter"]["description"])
assert conf["filters"]["testfilter"]["description"] == "nbr 1"
print("Set conf2, 3, 4, 2, 3, 4, ...")
for _ in range(100):
    print(".", end="", flush=True)
    cdsp.config.set_active_raw(configs[1])
    time.sleep(0.001)
    cdsp.config.set_active_raw(configs[2])
    time.sleep(0.001)
    cdsp.config.set_active_raw(configs[3])
    time.sleep(0.5)
    conf = cdsp.config.active()
    desc = conf["filters"]["testfilter"]["description"]
    assert conf["filters"]["testfilter"]["description"] == "nbr 4", f"{desc} != nbr 4"