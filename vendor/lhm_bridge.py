"""Isolated LibreHardwareMonitor reader (CPU package temp + power).

WHY A SEPARATE PROCESS: pythonnet can throw unhandled CLR exceptions
(System.NullReferenceException in Converter.ToPython when a sensor Value is
null) that instantly kill the host process with no traceback. Isolated here,
a bridge crash only kills the bridge; the supervisor (metrics.py) restarts it.
Output: vendor/lhm.json refreshed every ~2s (atomic replace).
"""
import json
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))  # vendor/
DLLDIR = os.path.join(HERE, "LHM")
OUT = os.path.join(HERE, "lhm.json")


def main():
    sys.path.insert(0, DLLDIR)
    import clr  # pythonnet
    clr.AddReference("LibreHardwareMonitorLib")
    from LibreHardwareMonitor.Hardware import Computer
    c = Computer()
    c.IsCpuEnabled = True
    c.Open()
    while True:
        temp = watts = None
        try:
            for hw in c.Hardware:
                try:
                    hw.Update()
                except Exception:
                    continue
                for s in hw.Sensors:
                    try:
                        st, nm = str(s.SensorType), s.Name or ""
                        v = s.Value
                        fv = float(v) if v is not None else None
                    except Exception:
                        continue
                    if fv is None:
                        continue
                    if "Temperature" in st and "Package" in nm and temp is None:
                        temp = fv
                    elif "Power" in st and "Package" in nm and watts is None:
                        watts = fv
        except Exception:
            pass
        try:
            tmp = OUT + ".tmp"
            with open(tmp, "w") as f:
                json.dump({"temp": temp, "watts": watts, "ts": time.time()}, f)
            os.replace(tmp, OUT)
        except Exception:
            pass
        time.sleep(2)


if __name__ == "__main__":
    main()
