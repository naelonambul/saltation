#!/usr/bin/env bash
# Builds and boots the saltation-oracle VM on VMware Workstation from the
# official Oracle Linux 8.10 KVM image + a separate mods.iso (OL8MODS) carrying
# the matching kernel-uek-modules RPM for VMware vmxnet3 support.
set -euo pipefail

OL8_BUILD="${OL8_BUILD:-b287}"
OL8_KVER="${OL8_KVER:-5.15.0-320.202.8.5.el8uek.x86_64}"
OL8_IMG_NAME="OL8U10_x86_64-kvm-${OL8_BUILD}.qcow2"
OL8_IMG_URL="${OL8_IMG_URL:-https://yum.oracle.com/templates/OracleLinux/OL8/u10/x86_64/${OL8_IMG_NAME}}"
MODS_RPM_NAME="kernel-uek-modules-${OL8_KVER}.rpm"
MODS_RPM_URL="${MODS_RPM_URL:-https://yum.oracle.com/repo/OracleLinux/OL8/UEKR7/x86_64/getPackage/${MODS_RPM_NAME}}"

VM_NAME="${VM_NAME:-saltation-oracle}"
VM_CPUS="${VM_CPUS:-4}"
VM_MEM_MB="${VM_MEM_MB:-8192}"
VM_DISK_SIZE="${VM_DISK_SIZE:-45G}"
WORK_DIR="${WORK_DIR:-$HOME/.cache/saltation-ol8}"
PUBKEY_FILE="${PUBKEY_FILE:-$HOME/.config/wsl-vmware-lab/lab_ed25519.pub}"
VMWARE_DIR="${VMWARE_DIR:-/mnt/c/Program Files/VMware/VMware Workstation}"

if [ -z "${VM_ROOT:-}" ]; then
  win_profile="$(/mnt/c/Windows/System32/cmd.exe /c "echo %USERPROFILE%" 2>/dev/null | tr -d '\r\n')"
  VM_ROOT="$(wslpath -u "$win_profile")/Documents/Virtual Machines"
fi
VM_DIR="${VM_ROOT}/${VM_NAME}"

for tool in curl qemu-img cloud-localds python3 wslpath; do
  command -v "$tool" >/dev/null 2>&1 || { echo "missing required tool: $tool" >&2; exit 2; }
done
ISO_TOOL="$(command -v genisoimage || command -v mkisofs || command -v xorrisofs || true)"
[ -n "$ISO_TOOL" ] || { echo "missing ISO builder (genisoimage, mkisofs, or xorrisofs)" >&2; exit 2; }
[ -f "$PUBKEY_FILE" ] || { echo "SSH public key not found: $PUBKEY_FILE" >&2; exit 2; }
[ -x "$VMWARE_DIR/vmrun.exe" ] || { echo "vmrun.exe not found in $VMWARE_DIR" >&2; exit 2; }

mkdir -p "$WORK_DIR/mods" "$VM_DIR"

if [ ! -f "$WORK_DIR/$OL8_IMG_NAME" ]; then
  echo "Downloading $OL8_IMG_URL ..."
  curl -fL --retry 3 -o "$WORK_DIR/$OL8_IMG_NAME.part" "$OL8_IMG_URL"
  mv "$WORK_DIR/$OL8_IMG_NAME.part" "$WORK_DIR/$OL8_IMG_NAME"
fi

if [ ! -f "$WORK_DIR/mods/$MODS_RPM_NAME" ]; then
  echo "Downloading $MODS_RPM_URL ..."
  curl -fL --retry 3 -o "$WORK_DIR/mods/$MODS_RPM_NAME.part" "$MODS_RPM_URL"
  mv "$WORK_DIR/mods/$MODS_RPM_NAME.part" "$WORK_DIR/mods/$MODS_RPM_NAME"
fi

if [ ! -f "$WORK_DIR/mods.iso" ]; then
  "$ISO_TOOL" -o "$WORK_DIR/mods.iso" -V OL8MODS -R -J "$WORK_DIR/mods/"
fi
cp "$WORK_DIR/mods.iso" "$VM_DIR/mods.iso"

if [ ! -f "$VM_DIR/${VM_NAME}.vmdk" ]; then
  cp "$WORK_DIR/$OL8_IMG_NAME" "$WORK_DIR/${VM_NAME}.qcow2"
  qemu-img resize -f qcow2 "$WORK_DIR/${VM_NAME}.qcow2" "$VM_DISK_SIZE"
  qemu-img convert -f qcow2 -O vmdk -o subformat=monolithicSparse "$WORK_DIR/${VM_NAME}.qcow2" "$VM_DIR/${VM_NAME}.vmdk"
  rm -f "$WORK_DIR/${VM_NAME}.qcow2"
fi

PUBKEY="$(cat "$PUBKEY_FILE")"
cat <<EOF > "$WORK_DIR/meta-data"
instance-id: ${VM_NAME}-ol8-01
local-hostname: ${VM_NAME}
EOF

cat <<'EOF' > "$WORK_DIR/user-data"
#cloud-config
hostname: __VM_NAME__
fqdn: __VM_NAME__
manage_etc_hosts: true
users:
  - name: lab
    groups: wheel
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    ssh_authorized_keys:
      - __PUBKEY__
ssh_pwauth: false
growpart:
  mode: auto
  devices: ["/"]
resize_rootfs: true
write_files:
  - path: /usr/local/sbin/saltation-ol8-net-init.sh
    permissions: "0755"
    content: |
      #!/bin/bash
      set -e
      {
        echo "=== OL8 MODS INIT START ==="
        if ! modinfo vmxnet3 >/dev/null 2>&1; then
          mkdir -p /mnt/ol8mods
          mount /dev/disk/by-label/OL8MODS /mnt/ol8mods
          rpm -ivh --nodeps /mnt/ol8mods/kernel-uek-modules-*.rpm
          umount /mnt/ol8mods
          rmdir /mnt/ol8mods
          depmod -a
        fi
        modprobe vmxnet3
        sleep 2
        for d in $(nmcli -t -f DEVICE,TYPE device | awk -F: '$2=="ethernet"{print $1}'); do
          nmcli connection show "$d" >/dev/null 2>&1 || nmcli connection add type ethernet ifname "$d" con-name "$d" ipv4.method auto
          nmcli connection up "$d" || true
        done
        sleep 4
        firewall-cmd --permanent --add-port=1521/tcp || true
        firewall-cmd --reload || true
        ip -4 -br addr
        echo "=== OL8 MODS INIT DONE ==="
      } | tee /dev/kmsg
runcmd:
  - /usr/local/sbin/saltation-ol8-net-init.sh
EOF

python3 - "$WORK_DIR/user-data" "$VM_NAME" "$PUBKEY" <<'PY'
from pathlib import Path
import sys
p, vm_name, pubkey = Path(sys.argv[1]), sys.argv[2], sys.argv[3]
p.write_text(p.read_text().replace("__VM_NAME__", vm_name).replace("__PUBKEY__", pubkey))
PY

cloud-localds "$VM_DIR/seed.iso" "$WORK_DIR/user-data" "$WORK_DIR/meta-data"

cat <<EOF > "$VM_DIR/${VM_NAME}.vmx"
.encoding = "UTF-8"
config.version = "8"
virtualHW.version = "19"
displayName = "${VM_NAME}"
guestOS = "oraclelinux8-64"
numvcpus = "${VM_CPUS}"
cpuid.coresPerSocket = "${VM_CPUS}"
memsize = "${VM_MEM_MB}"
firmware = "efi"
msg.autoAnswer = "TRUE"
uuid.action = "create"
pciBridge0.present = "TRUE"
pciBridge4.present = "TRUE"
pciBridge4.virtualDev = "pcieRootPort"
pciBridge4.functions = "8"
sata0.present = "TRUE"
sata0:0.present = "TRUE"
sata0:0.fileName = "${VM_NAME}.vmdk"
ide1:0.present = "TRUE"
ide1:0.deviceType = "cdrom-image"
ide1:0.fileName = "seed.iso"
ide1:0.startConnected = "TRUE"
ide1:1.present = "TRUE"
ide1:1.deviceType = "cdrom-image"
ide1:1.fileName = "mods.iso"
ide1:1.startConnected = "TRUE"
serial0.present = "TRUE"
serial0.fileType = "file"
serial0.fileName = "serial.log"
ethernet0.present = "TRUE"
ethernet0.startConnected = "TRUE"
ethernet0.virtualDev = "vmxnet3"
ethernet0.connectionType = "custom"
ethernet0.vnet = "VMnet8"
ethernet0.addressType = "generated"
ethernet1.present = "TRUE"
ethernet1.startConnected = "TRUE"
ethernet1.virtualDev = "vmxnet3"
ethernet1.connectionType = "custom"
ethernet1.vnet = "VMnet1"
ethernet1.addressType = "generated"
ethernet2.present = "TRUE"
ethernet2.startConnected = "TRUE"
ethernet2.virtualDev = "vmxnet3"
ethernet2.connectionType = "bridged"
ethernet2.addressType = "generated"
usb.present = "FALSE"
sound.present = "FALSE"
floppy0.present = "FALSE"
tools.syncTime = "TRUE"
EOF

rm -f "$VM_DIR/serial.log"
VMX_WIN="$(wslpath -w "$VM_DIR/${VM_NAME}.vmx")"
"$VMWARE_DIR/vmrun.exe" -T ws start "$VMX_WIN" nogui
echo "Started ${VM_NAME} (${VMX_WIN}). Inspect ${VM_DIR}/serial.log for assigned IP addresses."
