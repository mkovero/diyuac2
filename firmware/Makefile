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

# NXP SDK drivers used by the audio hardware layer (paths relative to TinyUSB).
SRC_C += \
	$(MCUX_CORE)/drivers/sai/fsl_sai.c \
	$(MCUX_CORE)/drivers/lpi2c/fsl_lpi2c.c
INC += \
	$(TOP)/$(MCUX_CORE)/drivers/sai \
	$(TOP)/$(MCUX_CORE)/drivers/lpi2c

# Bounded waits in the LPI2C driver instead of hanging on a stuck bus.
CFLAGS += -DI2C_RETRY_TIMES=100000U

include ../lib/tinyusb/hw/bsp/family_rules.mk
