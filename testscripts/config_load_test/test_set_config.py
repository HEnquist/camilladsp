import time
import camilladsp
import pytest



@pytest.fixture
def camillaclient():
    cdsp = camilladsp.CamillaClient("localhost", 1234)
    cdsp.connect()
    yield cdsp

# Read the CONFIGS
CONFIGS = []
for n in range(4):
    with open(f"conf{n+1}.yml") as f:
        CONFIGS.append(f.read())

def test_slow_via_ws(camillaclient):
    # Apply them all slowly
    print("Changing slowly")
    for n in range(4):
        print(f"Set conf{n+1}")
        camillaclient.config.set_active_raw(CONFIGS[n])
        time.sleep(2)
        conf = camillaclient.config.active()
        print("active:", conf["filters"]["testfilter"]["description"])
        assert conf["filters"]["testfilter"]["description"] == f"nbr {n+1}"


# Apply them with short delay
def change_with_delay(camillaclient, delay, reps):
    print(f"Changing with {1000*delay} ms delay")
    print("Set conf1")
    camillaclient.config.set_active_raw(CONFIGS[0])
    time.sleep(1)
    conf = camillaclient.config.active()
    print("active:", conf["filters"]["testfilter"]["description"])
    assert conf["filters"]["testfilter"]["description"] == "nbr 1"
    print("Set conf2, 3, 4, 2, 3, 4, ...")
    for _ in range(reps):
        print(".", end="", flush=True)
        camillaclient.config.set_active_raw(CONFIGS[1])
        time.sleep(delay)
        camillaclient.config.set_active_raw(CONFIGS[2])
        time.sleep(delay)
        camillaclient.config.set_active_raw(CONFIGS[3])
        time.sleep(0.5)
        conf = camillaclient.config.active()
        desc = conf["filters"]["testfilter"]["description"]
        assert conf["filters"]["testfilter"]["description"] == "nbr 4", f"{desc} != nbr 4"

def test_100ms_via_ws(camillaclient):
    change_with_delay(camillaclient ,0.1, 100)

def test_1ms_via_ws(camillaclient):
    change_with_delay(camillaclient, 0.001, 100)