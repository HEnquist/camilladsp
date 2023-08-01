import time
import camilladsp
import pytest
import os
import signal
import shutil
from subprocess import check_output


@pytest.fixture
def camillaclient():
    cdsp = camilladsp.CamillaClient("localhost", 1234)
    cdsp.connect()
    yield cdsp

@pytest.fixture
def cdsp_pid():
    res = check_output(["pgrep","camilladsp"])
    pid = int(res.decode())
    return pid

# Read the CONFIGS
CONFIGS = []
PATHS = []
for n in range(4):
    path = os.path.join(os.path.dirname(__file__), f"conf{n+1}.yml")
    PATHS.append(path)
    with open(path) as f:
        CONFIGS.append(f.read())

def test_slow_via_ws(camillaclient):
    # Apply them all slowly
    print("Changing slowly")
    for n in range(4):
        print(f"Set conf{n+1}")
        camillaclient.config.set_active_raw(CONFIGS[n])
        time.sleep(1)
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
    change_with_delay(camillaclient ,0.1, 10)

def test_1ms_via_ws(camillaclient):
    change_with_delay(camillaclient, 0.001, 10)

def test_slow_via_path(camillaclient):
    # Apply them all slowly
    print("Changing slowly")
    for n in range(4):
        print(f"Set conf{n+1}")
        camillaclient.config.set_file_path(PATHS[n])
        camillaclient.general.reload()
        time.sleep(1)
        conf = camillaclient.config.active()
        print("active:", conf["filters"]["testfilter"]["description"])
        assert conf["filters"]["testfilter"]["description"] == f"nbr {n+1}"

# Apply them with short delay
def change_path_with_delay(camillaclient, delay, reps):
    print(f"Changing with {1000*delay} ms delay")
    print("Set conf1")
    camillaclient.config.set_file_path(PATHS[0])
    camillaclient.general.reload()
    time.sleep(1)
    conf = camillaclient.config.active()
    print("active:", conf["filters"]["testfilter"]["description"])
    assert conf["filters"]["testfilter"]["description"] == "nbr 1"
    print("Set conf2, 3, 4, 2, 3, 4, ...")
    for _ in range(reps):
        print(".", end="", flush=True)
        camillaclient.config.set_file_path(PATHS[1])
        camillaclient.general.reload()
        time.sleep(delay)
        camillaclient.config.set_file_path(PATHS[2])
        camillaclient.general.reload()
        time.sleep(delay)
        camillaclient.config.set_file_path(PATHS[3])
        camillaclient.general.reload()
        time.sleep(0.5)
        conf = camillaclient.config.active()
        desc = conf["filters"]["testfilter"]["description"]
        assert conf["filters"]["testfilter"]["description"] == "nbr 4", f"{desc} != nbr 4"

def test_100ms_via_path(camillaclient):
    change_path_with_delay(camillaclient, 0.1, 10)

def test_1ms_via_path(camillaclient):
    change_path_with_delay(camillaclient, 0.001, 10)

def test_slow_via_sighup(camillaclient, cdsp_pid):
    # Apply them all slowly
    print("Changing slowly")
    path = os.path.join(os.path.dirname(__file__), f"temp.yml")
    shutil.copy(PATHS[0], path)
    camillaclient.config.set_file_path(path)
    for n in range(4):
        print(f"Set conf{n+1}")
        # copy config
        shutil.copy(PATHS[n], path)
        # send sighup
        os.kill(cdsp_pid, signal.SIGHUP)
        time.sleep(1)
        conf = camillaclient.config.active()
        print("active:", conf["filters"]["testfilter"]["description"])
        assert conf["filters"]["testfilter"]["description"] == f"nbr {n+1}"

# Apply them with short delay
def sighup_with_delay(camillaclient, cdsp_pid, delay, reps):
    print(f"Changing with {1000*delay} ms delay")
    print("Set conf1")
    path = os.path.join(os.path.dirname(__file__), f"temp.yml")
    shutil.copy(PATHS[0], path)
    camillaclient.config.set_file_path(path)
    os.kill(cdsp_pid, signal.SIGHUP)
    time.sleep(1)
    conf = camillaclient.config.active()
    print("active:", conf["filters"]["testfilter"]["description"])
    assert conf["filters"]["testfilter"]["description"] == "nbr 1"
    print("Set conf2, 3, 4, 2, 3, 4, ...")
    for _ in range(reps):
        print(".", end="", flush=True)
        # copy config
        shutil.copy(PATHS[1], path)
        # send sighup
        os.kill(cdsp_pid, signal.SIGHUP)
        time.sleep(delay)
        # copy config
        shutil.copy(PATHS[2], path)
        # send sighup
        os.kill(cdsp_pid, signal.SIGHUP)
        time.sleep(delay)
        # copy config
        shutil.copy(PATHS[3], path)
        # send sighup
        os.kill(cdsp_pid, signal.SIGHUP)
        time.sleep(0.5)
        conf = camillaclient.config.active()
        desc = conf["filters"]["testfilter"]["description"]
        assert conf["filters"]["testfilter"]["description"] == "nbr 4", f"{desc} != nbr 4"

def test_100ms_via_sighup(camillaclient, cdsp_pid):
    sighup_with_delay(camillaclient, cdsp_pid, 0.1, 10)

def test_1ms_via_sighup(camillaclient, cdsp_pid):
    sighup_with_delay(camillaclient, cdsp_pid, 0.001, 10)