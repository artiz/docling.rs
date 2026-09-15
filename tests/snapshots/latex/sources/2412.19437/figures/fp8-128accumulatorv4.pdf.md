Input

Scaling

Factor

Tensor Core

Output

CUDA Core

...

...

(a) Fine-grained quantization

Weight

Scaling

Factor

...

...

WGMMA 1

WGMMA 4

Tensor Core

Output

CUDA Core Low Prec Acc

/

GEMM Input

Interval

Scaling Factor

FP32 Register

(b) Increasing accumulation precision
