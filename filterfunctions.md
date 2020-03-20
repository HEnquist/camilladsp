# Building higher order filters with Biquads
The standard filters in CamillaDSP are not of a specific type, like Butterworth. Instead they are generic, with an adjustable q-value. To make the normal filters, one or several generic filters have to be used together.


# Bessel
Making a Bessel filter with a set of Biquads requires creating several Biquads, each with a unique Q and cut-off frequency.

## Multiplication factor for frequency:
| Order | Biquad 1   | Biquad 2  | Biquad 3  | Biquad 4 |
|-----------|-----|----|----|----|
| 1| 1.0*           |                |                |                |
| 2| 1.27201964951  |                |                |                |
| 3| 1.32267579991* | 1.44761713315  |                |                |
| 4| 1.60335751622  | 1.43017155999  |                |                |
| 5| 1.50231627145* | 1.75537777664  | 1.5563471223   |                |
| 6| 1.9047076123   | 1.68916826762  | 1.60391912877  |                |
| 7| 1.68436817927* | 2.04949090027  | 1.82241747886  | 1.71635604487  |
| 8| 2.18872623053  | 1.95319575902  | 1.8320926012   | 1.77846591177  |

The asterisk (*) indicates that this is a 1st order filter. 


## Q values:
| Order | Biquad 1   | Biquad 2  | Biquad 3  | Biquad 4 |
|-----------|-----|----|----|----|
| 1 | (1st order)   |                |               |              |
| 2 | 0.57735026919 |                |               |              |
| 3 | (1st order)   | 0.691046625825 |               |              |
| 4 | 0.805538281842| 0.521934581669 |               |              |
| 5 | (1st order)   | 0.916477373948 |0.563535620851 |              |
| 6 | 1.02331395383 | 0.611194546878 |0.510317824749 |              |
| 7 | (1st order)   | 1.12625754198  |0.660821389297 |0.5323556979  |
| 8 | 1.22566942541 | 0.710852074442 |0.559609164796 |0.505991069397|

## Example Bessel filter
Let's make a 5th order Lowpass at 1 kHz. Loking at the tables we see that we need three filters. The first should be a 1st order while the second and third are 2nd order.
- First filter, type LowpassFO:
  * freq = 1kHz * 1.50231627145 = 1502Hz
  * (no q-value)
- Second filter, type Lowpass:
  * freq = 1kHz * 1.75537777664 = 1755Hz
  * q = 0.916477373948
- Third filter, type Lowpass:
  * freq = 1kHz * 1.5563471223 = 1556Hz
  * q = 0.563535620851

# Butterworth and Linkwitz-Riley
For an Nth order Butterworth you will have N/2 biquad
sections if N is even, and ((N+1)/2 if N is odd.
For odd filters one of the Biquads will be a first order filter.
Each filter will have the same resonant frequency f0 and the second order filters will have Q according to this formula:
```
Q = 1/( 2*sin((pi/N)*(n + 1/2)) )
```
where `0 <= n < (N-1)/2`


## Table for q-values
Butterworth and Linkwitz-Riley filtes can easily be built with Biquads. The following table lists the most common ones. High- and lowpass use the same parameters.
| Type| Order   | Biquad 1   | Biquad 2  | Biquad 3  | Biquad 4 |
|-----------|-----|----|----|----|----
| Butterworth | 2   | 0.71 | 
|        | 4   | 0.54| 1.31 |
|        | 8   | 0.51| 0.6| 0.9 | 2.56 | 
| Linkwitz-Riley | 2   | 0.5 | 
|        | 4   | 0.71| 0.71 |
|        | 8   | 0.54| 1.31 | 0.54| 1.31 |

Note that a 4th order LR iconsists of two 2nd order Butterworth filters, and that an 8th order LR consists of two 4:th order Butterworth filters.