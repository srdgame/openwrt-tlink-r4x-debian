#!/bin/bash


macaddr_canonicalize() {
	local mac="$1"
	local canon=""

	mac=$(echo -n $mac | tr -d \")
	[ ${#mac} -gt 17 ] && return
	[ -n "${mac//[a-fA-F0-9\.: -]/}" ] && return

	for octet in ${mac//[\.:-]/ }; do
		case "${#octet}" in
		1)
			octet="0${octet}"
			;;
		2)
			;;
		4)
			octet="${octet:0:2} ${octet:2:2}"
			;;
		12)
			octet="${octet:0:2} ${octet:2:2} ${octet:4:2} ${octet:6:2} ${octet:8:2} ${octet:10:2}"
			;;
		*)
			return
			;;
		esac
		canon=${canon}${canon:+ }${octet}
	done

	[ ${#canon} -ne 17 ] && return

	printf "%02x:%02x:%02x:%02x:%02x:%02x" 0x${canon// / 0x} 2>/dev/null
}

macaddr_setbit() {
	local mac=$1
	local bit=${2:-0}

	[ $bit -gt 0 -a $bit -le 48 ] || return

	printf "%012x" $(( 0x${mac//:/} | 2**(48-bit) )) | sed -e 's/\(.\{2\}\)/\1:/g' -e 's/:$//'
}

macaddr_setbit_la() {
	macaddr_setbit $1 7
}

macaddr_unsetbit_mc() {
	local mac=$1

	printf "%02x:%s" $((0x${mac%%:*} & ~0x01)) ${mac#*:}
}

macaddr_add() {
	local mac=$1
	local val=$2
	local oui=${mac%:*:*:*}
	local nic=${mac#*:*:*:}

	nic=$(printf "%06x" $((0x${nic//:/} + val & 0xffffff)) | sed 's/^\(.\{2\}\)\(.\{2\}\)\(.\{2\}\)/\1:\2:\3/')
	echo $oui:$nic
}

macaddr_generate_from_mmc_cid() {
	local mmc_dev=$1

	local sd_hash=$(sha256sum /sys/class/block/$mmc_dev/device/cid)
	local mac_base=$(macaddr_canonicalize "$(echo "${sd_hash}" | dd bs=1 count=12 2>/dev/null)")
	echo "$(macaddr_unsetbit_mc "$(macaddr_setbit_la "${mac_base}")")"
}

tlink_mac_mask() {
	local mac="$1"
	local mac_base="b0c9"

	mac_val="${mac_base:0:2}:${mac_base:2:2}:${mac:6:11}"

	echo "${mac_val}"
	return 0
}

tlink_gen_mac() {
	local mmc_cid_mac=$(macaddr_generate_from_mmc_cid mmcblk1)
	local lan_mac=$(tlink_mac_mask "$mmc_cid_mac")
	echo "LAN MAC: "$lan_mac > /dev/kmsg

	sed -i "s/^MACAddress=.*/MACAddress=$lan_mac/" /etc/systemd/network/60-lan1.link
	sed -i "s/^MACAddress=.*/MACAddress=$(macaddr_add "$lan_mac" 1)/" /etc/systemd/network/60-lan2.link
	sed -i "s/^MACAddress=.*/MACAddress=$(macaddr_add "$lan_mac" 2)/" /etc/systemd/network/60-lan3.link
	sed -i "s/^MACAddress=.*/MACAddress=$(macaddr_add "$lan_mac" 3)/" /etc/systemd/network/60-lan4.link
}

echo "Hello, this is the first boot!" > /tmp/firstboot.log

tlink_gen_mac
