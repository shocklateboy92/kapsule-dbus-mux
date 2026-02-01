---
applyTo: '**'
---
This is running in a VM.
To deploy, you have to cd to `~/src/kde/src/kapsule` and run `./sysext.sh`.
You can access the VM host with `ssh 192.168.100.157`. If you need sudo, you can ssh as root.
The mux itself runs in a container called `m2`, with a tool called `kap`.
To get its logs, you can do `ssh 192.168.100.157 -- kap enter m2 -- journalctl --user -u kapsule-dbus-mux | tail`