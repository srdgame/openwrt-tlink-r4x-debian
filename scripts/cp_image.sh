# !/bin/bash
#
sudo mount /dev/sdc3 ./data
sudo cp *debian-2026*.img.zst ./data/openwrt-tlink-debian.img.zst
sync
sudo umount ./data
