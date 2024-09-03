import csv
import time
from datetime import datetime
from camilladsp import CamillaClient
from matplotlib import pyplot

cdsp = CamillaClient("localhost", 1234)
cdsp.connect()

loop_delay = 0.5

times = []
loads = []
levels = []

start = time.time()
start_time = datetime.now().strftime("%y.%m.%d_%H.%M.%S")

pyplot.ion()
fig = pyplot.figure()
ax1 = fig.add_subplot(211)
plot1, = ax1.plot([], [])
ax2 = fig.add_subplot(212)
plot2, = ax2.plot([], [])

running = True
try:
    while running:
        now = time.time()
        prc_load = cdsp.status.processing_load()
        buffer_level = cdsp.status.buffer_level()
        times.append(now - start)
        loads.append(prc_load)
        levels.append(buffer_level)
        #ax.plot(times, loads)
        plot1.set_data(times, loads)
        plot2.set_data(times, levels)
        ax1.relim()
        ax1.autoscale_view(True, True, True)
        ax2.relim()
        ax2.autoscale_view(True, True, True)
 
        # drawing updated values
        pyplot.draw()
        fig.canvas.draw()
        fig.canvas.flush_events()
        #pyplot.show()
        time.sleep(loop_delay)
        print(now)
except KeyboardInterrupt:
    print("stopping")
    pass

csv_name = f"loadlog_{start_time}.csv"
with open(csv_name, 'w', newline='') as f:
    writer = csv.writer(f)
    writer.writerow(["time", "load", "bufferlevel"])
    writer.writerows(zip(times, loads, levels)) 

print(f"saved {len(times)} records to '{csv_name}'")