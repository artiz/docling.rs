Transformer Block ×

𝐿𝐿

Feed-Forward Network

RMSNorm

Attention

RMSNorm Output Hidden 𝐡𝐡 DeepSeekMoE

0

1

...

𝑠𝑠

𝑁𝑁

′

𝑡𝑡 ... ...

1

2

Router

... 1 ...

Multi-Head Latent Attention (MLA) 0 Output Hidden 𝐮𝐮 𝑡𝑡

... ...

Multi-Head Attention concatenate

𝑄𝑄

𝐜𝐜 𝑡𝑡 apply RoPE

{[ 𝐪𝐪 𝑡𝑡 , 𝑖𝑖 𝐶𝐶 ; 𝐪𝐪 𝑡𝑡 , 𝑖𝑖 𝑅𝑅 ]}

{[ 𝐤𝐤 𝑡𝑡 , 𝑖𝑖 𝐶𝐶 ; 𝐤𝐤 𝑡𝑡 𝑅𝑅 ]}

RoPE

{ 𝐪𝐪 𝑡𝑡 , 𝑖𝑖 𝐶𝐶 } { 𝐪𝐪 𝑡𝑡 , 𝑖𝑖 𝑅𝑅 }

{ 𝐯𝐯

...

𝑡𝑡 , 𝑖𝑖 𝐶𝐶 } { 𝐤𝐤 𝑡𝑡 , 𝑖𝑖 𝐶𝐶 }

𝐤𝐤 𝑡𝑡 𝑅𝑅 concatenate apply

Latent

...

Latent

Input Hidden 𝐡𝐡 𝑡𝑡

... ...

3

4

Routed Expert

Shared Expert

...

Top- Input Hidden

𝐾𝐾 𝑟𝑟

𝑁𝑁 𝑟𝑟 -1 𝑁𝑁

𝑟𝑟

𝐮𝐮

𝑡𝑡

Cached During Inference 𝐜𝐜

𝑡𝑡 𝐾𝐾𝐾𝐾
