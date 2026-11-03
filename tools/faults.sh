#!/bin/sh
# Fault injection against a running diyuac2 (Linux host, run as a user with
# sudo). Each case breaks or stresses the stream, then checks the device is
# back at high speed and that a clean 5 s playback + capture still runs
# with zero error counters.
#
# Not covered: SET_CUR rate changes and CLEAR_FEATURE(ENDPOINT_HALT) while
# streaming. usbfs refuses class and endpoint requests to interfaces that
# snd-usb-audio holds (only vendor requests pass).
#
#   faults.sh [case...]    cases: reset port suspend openclose order all
set -u
CARD=${CARD:-3}
VC="sudo -n python3 $(dirname "$0")/vendorctl.py"
FAIL=0

dev() {    # sysfs path of the device
	for d in /sys/bus/usb/devices/*; do
		[ "$(cat "$d/idVendor" 2>/dev/null)$(cat "$d/idProduct" 2>/dev/null)" = cafe0ac2 ] && { echo "$d"; return; }
	done
}

wait_dev() {    # wait until enumerated and the ALSA card is back
	i=0
	while [ $i -lt 100 ]; do
		d=$(dev)
		if [ -n "$d" ] && [ -e /proc/asound/card$CARD/id ] && $VC info >/dev/null 2>&1; then
			return 0
		fi
		sleep 0.1; i=$((i + 1))
	done
	return 1
}

play() {    # play [secs]: 1 kHz sine, raw (no WAV size limit)
	sox -n -r 48000 -c 2 -b 32 -e signed -t raw - synth "$1" sine 1000 vol 0.1 2>/dev/null |
		aplay -q -D hw:$CARD,0 -t raw -f S32_LE -c 2 -r 48000 2>>/tmp/faults_alsa.err
}

rec() {
	arecord -q -D hw:$CARD,0 -t raw -f S32_LE -c 2 -r 48000 -d "$1" /dev/null 2>>/tmp/faults_alsa.err
}

check() {    # check <case>: speed, clean stream, zero counters
	name=$1
	if ! wait_dev; then
		echo "FAIL $name: device did not come back"; FAIL=1; return
	fi
	speed=$(cat "$(dev)/speed")
	$VC clear-stats >/dev/null
	rec 5 & play 5; wait
	st=$($VC stats)
	if [ "$speed" = 480 ] && echo "$st" | grep -q "underruns 0 rec_overruns 0 rec_drops 0 sai_fifo_err tx 0 rx 0"; then
		echo "ok   $name (speed $speed) ${st#* }"
	else
		echo "FAIL $name (speed $speed) $st"; FAIL=1
	fi
}

stream_bg() {    # stream_bg <secs>: playback + capture in the background
	rec "$1" & play "$1" &
}

case_reset() {    # USB bus reset (usbreset) in the middle of a stream
	stream_bg 6; sleep 2
	sudo -n usbreset cafe:0ac2 >/dev/null
	wait
	check reset
}

case_port() {    # logical unplug/replug: disable the hub port mid-stream
	name=$(basename "$(dev)")        # 9-1 or 9-1.2
	num=${name##*[.-]}
	parent=${name%[.-]*}
	case $name in
	*.*) p=/sys/bus/usb/devices/$parent/$parent:1.0/$parent-port$num ;;
	*) p=/sys/bus/usb/devices/usb$parent/$parent-0:1.0/usb$parent-port$num ;;
	esac
	[ -e "$p/disable" ] || { echo "skip port: no $p/disable"; return; }
	stream_bg 6; sleep 2
	echo 1 | sudo -n tee "$p/disable" >/dev/null; sleep 1
	echo 0 | sudo -n tee "$p/disable" >/dev/null
	wait
	check port
}

case_suspend() {    # runtime suspend while idle, resume by opening the stream
	d=$(dev)
	echo 500 | sudo -n tee "$d/power/autosuspend_delay_ms" >/dev/null
	echo auto | sudo -n tee "$d/power/control" >/dev/null
	i=0
	while [ "$(cat "$d/power/runtime_status")" != suspended ] && [ $i -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
	st=$(cat "$d/power/runtime_status")
	check "suspend (was $st)"
	echo on | sudo -n tee "$(dev)/power/control" >/dev/null
}

case_openclose() {    # 100 short open/close cycles, mixed directions
	i=0
	while [ $i -lt 100 ]; do
		case $((i % 3)) in
		0) play 0.05 ;;
		1) rec 1 & play 0.05; wait ;;
		2) timeout 0.3 arecord -q -D hw:$CARD,0 -t raw -f S32_LE -c 2 -r 48000 /dev/null 2>/dev/null ;;
		esac
		i=$((i + 1))
	done
	check openclose
}

case_order() {    # capture first then playback, playback first then capture, overlapping stops
	$VC clear-stats >/dev/null
	rec 4 & sleep 1; play 2; wait
	play 4 & sleep 1; rec 2; wait
	rec 3 & play 1; sleep 1; play 1; wait
	st=$($VC stats)
	echo "     order counters: ${st#* }"
	check order
}

: >/tmp/faults_alsa.err
cases=${*:-all}
[ "$cases" = all ] && cases="reset port suspend openclose order"
for c in $cases; do "case_$c"; done
echo "ALSA errors (expected only for reset/port): $(grep -c . /tmp/faults_alsa.err)"
exit $FAIL
