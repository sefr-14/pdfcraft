"""W0 a11y probe: print every AT-SPI application whose name starts with argv[1], with its
top-level windows and up to `depth` levels of descendants (role and name). Run with the
system python3 that has pyatspi (python3.12 on Ubuntu 24.04), on the session bus where the spike runs."""
import sys
import pyatspi

prefix = sys.argv[1] if len(sys.argv) > 1 else "w0"
depth_max = int(sys.argv[2]) if len(sys.argv) > 2 else 6
desktop = pyatspi.Registry.getDesktop(0)
apps = [a for a in desktop if a is not None]
print(f"ATSPI applications: {[a.name for a in apps]}")

def walk(node, depth):
    try:
        role, name, n = node.getRoleName(), node.name, node.childCount
    except Exception as e:  # stale node
        print("  " * depth + f"<error {e}>")
        return
    print("  " * depth + f"[{role}] {name!r} children={n}")
    if depth >= depth_max:
        return
    for i in range(n):
        walk(node.getChildAtIndex(i), depth + 1)

for app in apps:
    if app.name.lower().startswith(prefix):
        print(f"APP {app.name!r}: top-level windows={app.childCount}")
        for i in range(app.childCount):
            walk(app.getChildAtIndex(i), 1)
