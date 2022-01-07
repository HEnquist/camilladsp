## Raspberry pi 4, aarch64

### 192 kHz
- 8 channels
- chunksize: 4096
- 262144 taps
- no resampling
- overloaded

- 8 channels
- chunksize: 8192
- 262144 taps
- no resampling
- 71%

- 8 channels
- chunksize: 16384
- 262144 taps
- no resampling
- 58%

- 8 channels
- chunksize: 32768
- 262144 taps
- no resampling
- 55%

- 8 channels
- chunksize: 16384
- 262144 taps
- Synchronous resampling 
- 63%

- 8 channels
- chunksize: 16384
- 262144 taps
- FastAsync resampling 
- 68%

- 8 channels
- chunksize: 16384
- 262144 taps
- BalancedAsync resampling 
- 82%

- 8 channels
- chunksize: 16384
- 262144 taps
- AccurateAsync resampling 
- 156%

