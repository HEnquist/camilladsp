## Description of error messages
### Config files
- Could not open config file 'examplefile.yml'. Error: *description from OS*

  The specified file could not be opened. The description from the OS may give more info.

- Could not read config file 'examplefile.yml'. Error: *description from OS*

  The specified file could be opened but not read. The description from the OS may give more info.

- Invalid config file! *error description from Yaml parser*

  The config file is invalid Yaml. The error from the Yaml parser is printed in the next line.

### Config options
- target_level can't be larger than *1234*,

  Target level can't be larger than twice the chunksize.


### Pipeline
- Use of missing mixer '*mixername*'

  The pipeline lists a mixer named "mixername", but the corresponding definition doesn't exist in the "Mixers" section.

- Mixer '*mixername*' has wrong number of input channels. Expected *X*, found *Y*.

  This means that there is a mismatch in the number of channels. The number of input channels of a mixer 
  must match the number of output channels of the previous step in the pipeline. If there is only one mixer, 
  or this mixer is the first one, then the input channels must match the number of channels of the capture device.



- Pipeline outputs *X* channels, playback device has *Y*.

  This means that there is a mismatch in the number of channels. The number of channels of the playback device 
  must match the number of output channels of the previous step in the pipeline. If the pipeline doesn't contain any mixer, then the playback device must have the same number of channels as the capture device. If there is one or more mixers, then the output channels of the last mixer must match the number of channels of the playback device.
  
- Use of missing filter '*filtername*' 

  The pipeline lists a filter named "filtername", but the corresponding definition doesn't exist in the "Filters" section.

- Use of non existing channel *X*

  A filter step was defined that tries to filter a non-existing channel. 
  The available channel numbers are 0 to X-1, where X is the active number of channels. If there is no Mixer in front 
  of this filter, then X is the number of channels of the capture device. If there is a mixer, then X is 
  the number of output channels of that mixer.

### Filters

- Invalid filter '*filtername*'. Reason: *description from parser*

  The definition of the mixer is somehow wrong. The "Reason" should give more info.

  conv filter:
- Conv coefficients are empty
  
  The coefficient file for a filter was found to be empty.

- Could not open coefficient file '*examplefile.raw*'. Error:  *description from OS*

  The specified file could not be opened. The description from the OS may give more info.

- Can't parse value on line *X* of file '*examplefile.txt*'. Error: *description from parser*

  The value on the specified line could not be parsed as a number. Check that the file only contains numbers.

- Unstable filter specified

  This means that a Biquad filter definition was found to give an unstable filter, 
  meaning that the output signal can grow uncontrolled. Check that the coeffients were entered correctly.

- Negative delay specified

  The Delay filter can only provide positive delays.

### Mixers

- Invalid mixer '*mixername*'. Reason: *description from parser*
  
  The definition of the mixer is somehow wrong. The "Reason" should give more info.

- Invalid destination channel *X*, max is *Y*.
  
  A mapping was defined that tries to use a non-existing output channel. 
  The available destination channel numbers are 0 to output channels - 1.

- Invalid source channel *X*, max is *Y*.

  A mapping was defined that tries to use a non-existing input channel. 
  The available source channel numbers are 0 to input channels - 1.

