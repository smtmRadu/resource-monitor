# One-time setup: Resource Monitor starts elevated at every logon, no UAC prompt.
$py = "C:\Users\radup\AppData\Local\Programs\Python\Python312\pythonw.exe"
$app = "C:\Development\resource-monitor\app.py"
schtasks /create /tn "ResourceMonitor" /tr "`"$py`" `"$app`"" /sc onlogon /delay 0000:15 /rl highest /f
schtasks /run /tn "ResourceMonitor"
