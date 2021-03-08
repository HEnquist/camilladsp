# Converting coefficients from wav to raw

CamillaDSP needs the coefficients for the FIR filters as raw data. If you have an the coefficients (aka impulse response) in wav-format, this must first be converted to raw data before it can be used. If the wav contains more than one cannel, it also needs to be split to one channel per file. 

## Using sox
The conversion can be done on the command line using `sox`. 

Convert a mono wav file to a 32-bit raw file:
```sh
sox coeffs_mono.wav --bits 32 coeffs.raw
```

Convert a stereo wav file into separate raw files for left and right:
```sh
sox coeffs_stereo.wav --bits 32 coeffs_left.raw remix 1
sox coeffs_stereo.wav --bits 32 coeffs_right.raw remix 2
```

## Using Audacity
[Audacity](https://www.audacityteam.org/) can export audio data as raw files.

To convert a stereo wav file, follow these steps:
1) Open the wav file
2) [Split the stereo track into two mono tracks](https://manual.audacityteam.org/man/splitting_and_joining_stereo_tracks.html).
3) Select the upper track, [and export it as raw samples](https://manual.audacityteam.org/man/other_uncompressed_files_export_options.html)
   - In the Header dropdown, select "RAW (header-less)"
   - In the Encoding dropdown, select the format you want to use, for example "Signed 32-bit PCM" for 32 bit singed integers, or "32-bit float" for 32-bit float format.
   - Give a suitable filename for the left track and click "save". 
   - If the "Edit Metadata Tags" dialog pops up, just click "Ok".
4) Repeat step 3 for the lower track.

## Checking the result
Audacity can also read raw files, and this can be used to verify thatt he exported file looks reasonable.
Just use the [File / Import / Raw Data](https://manual.audacityteam.org/man/file_menu_import.html) function. Select the same encoding as when saving, and little-endian byte order. 





