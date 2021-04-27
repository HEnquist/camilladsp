# Converting a wav file to raw data

The `File` capture device of CamillaDSP can only read raw data. If you want feed it a wav file, this must first be converted to raw data before it can be used. 

## Using sox
The conversion can be done on the command line using `sox`. 

Convert a wav file to a 32-bit raw file:
```sh
sox example.wav --bits 32 example.raw
```

## Using Audacity
[Audacity](https://www.audacityteam.org/) can export audio data as raw files.

To convert a wav file, follow these steps:
1) Open the wav file
2) [and export it as raw samples](https://manual.audacityteam.org/man/other_uncompressed_files_export_options.html)
   - In the Header dropdown, select "RAW (header-less)"
   - In the Encoding dropdown, select the format you want to use, for example "Signed 32-bit PCM" for 32 bit singed integers, or "32-bit float" for 32-bit float format.
   - Give a suitable filename and click "save". 
   - If the "Edit Metadata Tags" dialog pops up, just click "Ok".

## Checking the result
Audacity can also read raw files, and this can be used to verify that the exported file looks reasonable.
Just use the [File / Import / Raw Data](https://manual.audacityteam.org/man/file_menu_import.html) function. Select the same encoding as when saving, and little-endian byte order. 





