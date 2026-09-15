Input

BF16

Input

Gradient To FP8

To BF16

Σ

FP32

Fprop

Σ

FP32

Weight

Dgrad To BF16

To FP8

To FP8

Output

Output

Gradient

BF16

或者 Input-&gt;Activation\_L

Output-&gt;Activation\_{L+1}

To FP8

To FP8

Wgrad

Σ

FP32

Master

Weight To FP32

Weight

Gradient

FP32

Optimizer

States To BF16
