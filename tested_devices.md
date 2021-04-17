# Tested devices

The following devices have been reported to work well for multichannel output with CamillaDSP.

Only devices providing more than 2 channels are listed.

If a device isn't listed it doesn't mean that it isn't working, just that there is no information yet. The same applied to empty fields in the table. 

| Device | Outputs | Linux | Windows | Macos | Comments |
| ------ | ------- | ----- | ------- | ----- | -------- |
| Okto Research dac8 Pro | 8 | OK<sup>a</sup> | | | a) Some firmware versions give trouble under Linux |
| Sound Blaster X-Fi Surround 5.1  | 6 | OK<sup>a</sup>  | OK | | a) The Linux driver only supports 48 kHz when using 6 channels |
| Asus Xonar U7 MK2 | 8 | OK | OK| |  |
| Asus Xonar U5 | 6 | OK | | |  |
| RME Digiface USB | 10<sup>a</sup>  | no | OK | OK | a) Two analog channels intended for headphones, the rest are digital only. 8 digital channels in AES/spdif mode, up to 32 in ADAT mode |
| DIYINHK DXIO32ch USB to I2S interface | 8<sup>a</sup>  | OK | OK |  | Digital (I2S) output only. a) 8 channels with standard firmware, up to 32 with alternate firmware using TDM mode |
| Focusrite Scarlett 18i20 gen1 | 20 | OK |  |  |  |
| Focusrite Scarlett 4i4 gen? | 4 | OK |  |  |  |