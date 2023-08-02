import time
import camilladsp
import pytest
import os
import signal
import shutil
from subprocess import check_output

# ---------- Constants -----------

CONFIGS = []
PATHS = []
for n in range(4):
    path = os.path.join(os.path.dirname(__file__), f"conf{n+1}.yml")
    PATHS.append(path)
    with open(path) as f:
        CONFIGS.append(f.read())
TEMP_PATH = os.path.join(os.path.dirname(__file__), f"temp.yml")


# ---------- Test fixtures -----------

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


# ---------- Helper functions -----------

def assert_active(cdsp, expected_desc):
    conf = cdsp.config.active()
    desc = conf["filters"]["testfilter"]["description"]
    assert desc == expected_desc

def set_via_sighup(pid, index):
    # copy config
    shutil.copy(PATHS[index], TEMP_PATH)
    # send sighup
    os.kill(pid, signal.SIGHUP)

def set_via_path(client, index):
    client.config.set_file_path(PATHS[index])
    client.general.reload()


# ---------- Test sending a config via ws -----------

def test_slow_via_ws(camillaclient):
    # Apply them all slowly
    print("Changing slowly")
    for n in range(4):
        print(f"Set conf{n+1}")
        camillaclient.config.set_active_raw(CONFIGS[n])
        time.sleep(1)
        assert_active(camillaclient, f"nbr {n+1}")

# Apply them with short delay
@pytest.mark.parametrize("delay,reps", [(0.1, 50), (0.01, 50), (0.001, 50)])
def test_set_via_ws(camillaclient, delay, reps):
    print(f"Changing with {1000*delay} ms delay")
    print("Set conf1")
    camillaclient.config.set_active_raw(CONFIGS[0])
    time.sleep(1)
    assert_active(camillaclient, "nbr 1")
    print("Set conf2, 3, 4, 2, 3, 4, ...")
    for r in range(reps):
        print("repetition", r)
        camillaclient.config.set_active_raw(CONFIGS[1])
        time.sleep(delay)
        camillaclient.config.set_active_raw(CONFIGS[2])
        time.sleep(delay)
        camillaclient.config.set_active_raw(CONFIGS[3])
        time.sleep(0.5)
        assert_active(camillaclient, "nbr 4")


# ---------- Test changing config by changing config path and reloading -----------

def test_slow_via_path(camillaclient):
    # Apply them all slowly
    print("Changing slowly")
    for n in range(4):
        print(f"Set conf{n+1}")
        camillaclient.config.set_file_path(PATHS[n])
        camillaclient.general.reload()
        time.sleep(1)
        assert_active(camillaclient, f"nbr {n+1}")

# Apply them with short delay
@pytest.mark.parametrize("delay,reps", [(0.1, 50), (0.01, 50), (0.001, 50)])
def test_set_via_path(camillaclient, delay, reps):
    print(f"Changing with {1000*delay} ms delay")
    print("Set conf1")
    set_via_path(camillaclient, 0)
    time.sleep(1)
    assert_active(camillaclient, "nbr 1")
    print("Set conf2, 3, 4, 2, 3, 4, ...")
    for r in range(reps):
        print("repetition", r)
        set_via_path(camillaclient, 1)
        time.sleep(delay)
        set_via_path(camillaclient, 2)
        time.sleep(delay)
        set_via_path(camillaclient, 3)
        time.sleep(0.5)
        assert_active(camillaclient, "nbr 4")

# ---------- Test changing config by updating the file and sending SIGHUP -----------

def test_slow_via_sighup(camillaclient, cdsp_pid):
    shutil.copy(PATHS[0], TEMP_PATH)
    camillaclient.config.set_file_path(TEMP_PATH)
    for n in range(4):
        print(f"Set conf{n+1}")
        set_via_sighup(cdsp_pid, n)
        time.sleep(1)
        assert_active(camillaclient, f"nbr {n+1}")

# Apply them with short delay
@pytest.mark.parametrize("delay,reps", [(0.1, 50), (0.01, 50), (0.001, 50)])
def test_set_via_sighup(camillaclient, cdsp_pid, delay, reps):
    print(f"Changing with {1000*delay} ms delay")
    print("Set conf1")
    camillaclient.config.set_file_path(TEMP_PATH)
    set_via_sighup(cdsp_pid, 0)
    time.sleep(1)
    assert_active(camillaclient, "nbr 1")
    print("Set conf2, 3, 4, 2, 3, 4, ...")
    for r in range(reps):
        print("repetition", r)
        set_via_sighup(cdsp_pid, 1)
        time.sleep(delay)
        set_via_sighup(cdsp_pid, 2)
        time.sleep(delay)
        set_via_sighup(cdsp_pid, 3)
        time.sleep(0.5)
        assert_active(camillaclient, "nbr 4")
