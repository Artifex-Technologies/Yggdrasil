# Task Manager

Yggdrasil's task manager (`yggdrasil-taskmanager`), laid out like the one on Windows 11, in the desktop's look (light or dark,
the accent colour from Settings). Ctrl+Shift+Esc opens it; pressing it again brings the open one to the front.

| Tab | What it shows and does |
|:--|:--|
| Processes | Apps (a window's process with all its helpers), background processes and the system's, with CPU, memory and disk shaded by how busy they are. End task, force it to end, suspend and resume, bring to front, close its windows, properties, search online |
| Performance | Live graphs over a minute: CPU (overall or each logical processor), memory, each disk, each network adapter, the graphics card (NVIDIA through nvidia-smi, AMD from its driver's files) |
| App history | Processor time and disk read and written for each app since it started |
| Startup apps | Autostart entries and the background services switched on in your account: enable or disable them |
| Users | Who is signed in, with their processor and memory use and apps |
| Details | Every process: PID, status, account, CPU, memory, threads, priority (set it), command line |
| Services | The system's services and yours: start, stop, restart, start automatically or not |

Run new task starts a program, opens a folder or a web address, with administrator rights when asked (pkexec).

How it reads: a background thread looks at /proc once a second (twice, every 4 seconds or paused, at the bottom of the sidebar),
so the window stays smooth; about 3 % of one processor core while it is open, nothing when closed. The graphics card is asked
only while the Performance tab shows (nvidia-smi costs a few hundredths of a second), services and sign-ins only while their
tab does. Memory is a process's own (resident minus shared), so an app's helper processes add up without counting shared
libraries twice. Another account's processes are ended through polkit (it asks for your password), and ending one that
belongs to the system or the desktop session asks first.

Ideas from [task-manager-for-mac](https://github.com/yatindma/task-manager-for-mac) (MIT, Swift): the seven tabs and the
heat shading; no code from it (different language and system).
