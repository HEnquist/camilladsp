import csv
import time
from datetime import datetime
from camilladsp import CamillaClient
from matplotlib import pyplot

cdsp = CamillaClient("localhost", 1234)
cdsp.connect()

loop_delay = 0.5
plot_interval = 10

times = []
loads = []
levels = []

start = time.time()
start_time = datetime.now().strftime("%y.%m.%d_%H.%M.%S")

pyplot.ion()
fig = pyplot.figure()
ax1 = fig.add_subplot(311)
plot1, = ax1.plot([], [])
ax2 = fig.add_subplot(312)
ax3 = fig.add_subplot(313)
plot3, = ax3.plot([], [])


running = True
plot_counter = 0
try:
    while running:
        now = time.time()
        prc_load = cdsp.status.processing_load()
        buffer_level = cdsp.status.buffer_level()
        times.append(now - start)
        loads.append(prc_load)
        levels.append(buffer_level)
        plot_counter += 1
        if plot_counter > plot_interval: 
            plot_counter = 0
            #ax.plot(times, loads)
            plot1.set_data(times, loads)
            plot3.set_data(times, levels)
            ax1.relim()
            ax1.autoscale_view(True, True, True)
            ax3.relim()
            ax3.autoscale_view(True, True, True)
            ax2.cla()
            ax2.hist(loads)

            # drawing updated values
            pyplot.draw()
            fig.canvas.draw()
            fig.canvas.flush_events()
            print(now)
        #pyplot.show()
        time.sleep(loop_delay)
except KeyboardInterrupt:
    print("stopping")
    pass

csv_name = f"loadlog_{start_time}.csv"
with open(csv_name, 'w', newline='') as f:
    writer = csv.writer(f)
    writer.writerow(["time", "load", "bufferlevel"])
    writer.writerows(zip(times, loads, levels)) 

print(f"saved {len(times)} records to '{csv_name}'")