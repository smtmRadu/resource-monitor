"""Embedded CPU temp/power via LibreHardwareMonitorLib (no GUI window)."""
import ctypes
import os
import sys

THEN = None
try:
    import clr  # pythonnet
    dll = os.path.join("vendor", "LHM", "LibreHardwareMonitorLib.dll")
    clr.AddReference(os.path.abspath(dll).replace(".dll", ""))
    from LibreHardwareMonitor.Hardware import Computer

    c = Computer()
    c.IsCpuEnabled = True
    c.Open()
    time_started = True
except Exception as e:
    print("INIT_FAIL:", repr(e))
    sys.exit(1)

found = []
import time
for hw in c.Hardware:
    hw.Update()
time.sleep(1.0)
for hw in c.Hardware:
    hw.Update()
    for s in hw.Sensors:
        st = str(s.SensorType)
        if ("Temperature" in st and "Package" in s.Name) or ("Power" in st and "Package" in s.Name):
            found.append((hw.Name, s.Name, st, s.Value))
    for sub in hw.SubHardware:
        sub.Update()
        for s in sub.Sensors:
            st = str(s.SensorType)
            if ("Temperature" in st and "Package" in s.Name) or ("Power" in st and "Package" in s.Name):
                found.append((hw.Name + "/" + sub.Name, s.Name, st, s.Value))

for row in found:
    print(row)
print("EMBEDDED_OK" if found else "NO_CPU_SENSORS")
c.Close()
