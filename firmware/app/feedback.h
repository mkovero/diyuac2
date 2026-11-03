// diyuac2 playback rate feedback, measured from the audio interface.
//
// See docs/feedback.md for the mechanism and why it was chosen.

#ifndef FEEDBACK_H_
#define FEEDBACK_H_

#include <stdint.h>

// Restart measurement (stream start, rate change). Task context.
void feedback_reset(uint32_t rate);

// Nominal 16.16 samples per microframe for the rate.
uint32_t feedback_nominal(uint32_t rate);

// Last value sent to the host, 16.16 samples per microframe.
uint32_t feedback_last(void);

#endif
