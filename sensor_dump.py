"""List every sensor LibreHardwareMonitor sees (enumeration needs no driver)."""
import os
import sys

sys.path.insert(0, os.path.join("vendor", "LHM"))
import clr
clr.AddReference("LibreHardwareMonitorLib")
from LibreHardwareMonitor.Hardware import Computer

c = Computer()
c.IsCpuEnabled = True
c.IsGpuEnabled = True
c.IsMemoryEnabled = True
c.Open()
for hw in c.Hardware:
    print("HW:", hw.Name, "|", hw.HardwareType)
    try:
        hw.Update()
    except Exception as e:
        print("  update note:", e)
    for s in hw.Sensors:
        print(f"  [{s.SensorType}] {s.Name} = {s.Value}")
c.Close()
print("DUMP_OK")
