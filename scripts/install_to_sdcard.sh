# !/bin/bash
# 安装生成的system镜像文件到sd card上
DEFAULT_IMG_TYP="full" 

SD_DEV=$1
IMG_TYP=${2:-$DEFAULT_IMG_TYP}

S_TYP="r4x"

echo ""
date
echo -e "\033[36m==============================="
echo "Install ${IMG_TYP} system image to sd-card ${SD_DEV}"
echo -e "===============================\033[37m"
echo ""

if [ ! -e ${SD_DEV} ]; then
	echo "ERROR  sdcard not exits."
	exit 1
fi

IMG_NAME="-${IMG_TYP}"
if [ "$IMG_TYP" == "full" ]; then
	IMG_NAME=""
fi

echo "Install system-${IMG_TYP} image to SD Card"
#echo "zstd -d -c openwrt-tlink-${S_TYP}-debian${IMG_NAME}-202*.img.zst | sudo dd of=${SD_DEV} bs=4M conv=fsync"
zstd -d -c openwrt-tlink-${S_TYP}-debian${IMG_NAME}-202*.img.zst | sudo dd of=${SD_DEV} bs=4M conv=fsync

if [ $? -ne 0 ]; then
	echo "ERROR install ${IMG_TYP} image to sdcard."
	exit 1
fi

echo "Create fstab mount point ..."
mkdir -p ./emmc
sudo mount ${SD_DEV}2 ./emmc
sudo mkdir -p ./emmc/mnt/data
echo "/dev/mmcblk0p3 /mnt/data auto defaults 0 0" | sudo tee -a ./emmc/etc/fstab
sync
sudo umount ./emmc
if [ $? -ne 0 ]; then
	echo $?
	sync
	sudo umount ./emmc
fi
rmdir ./emmc

echo "Create data partition"

echo -e "n\np\n3\n4000000\n\nt\n3\n83\nw" | sudo fdisk ${SD_DEV}  > /dev/null 2>&1
if [ $? -ne 0 ]; then
	echo "ERROR create data partition."
	exit 1
fi

sudo mkfs.ext4 -L data ${SD_DEV}3 > /dev/null 2>&1
if [ $? -ne 0 ]; then
	echo "ERROR formating ext4 partition."
	exit 1
fi

echo "  linux partition formated."

echo "Copy system image to data parition"
mkdir -p ./data
sudo mount ${SD_DEV}3 ./data
sudo cp openwrt-tlink-${S_TYP}-debian${IMG_NAME}-202*.img.zst ./data/openwrt-tlink-debian.img.zst
tree ./data/
sync
sudo umount ./data
if [ $? -ne 0 ]; then
	sleep 5
	sudo umount ./data
fi
rmdir ./data

echo ""
echo -e "\033[36m*******************************"
echo "Linux system installed to EMMC."
echo -e "*******************************\033[37m"
echo ""

exit 0
