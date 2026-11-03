# diyuac2 firmware, built out-of-tree on top of TinyUSB's make build system.
#
#   make BOARD=mimxrt1020_evk          # build
#   make BOARD=mimxrt1020_evk get-deps # one-time: fetch NXP SDK etc. into lib/tinyusb
#
# Application sources live in app/ (not src/, to avoid shadowing TinyUSB's
# own src/ tree, which is found through vpath).

BOARD ?= mimxrt1020_evk
PROJECT = diyuac2

include ../lib/tinyusb/hw/bsp/family_support.mk

INC += app

SRC_C += $(wildcard app/*.c)

include ../lib/tinyusb/hw/bsp/family_rules.mk
