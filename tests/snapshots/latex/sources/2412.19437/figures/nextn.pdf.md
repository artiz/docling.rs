Target Tokens

Input Tokens Cross-Entropy Loss Cross-Entropy Loss Cross-Entropy Loss 𝑡𝑡 3 𝑡𝑡 4 𝑡𝑡 𝑡𝑡 2

Main Model

(Next Token Prediction)

Transformer Block

Output Head × 𝐿𝐿

× 𝐿𝐿

× 𝐿𝐿

× 𝐿𝐿

Transformer Block Transformer Block Transformer Block Transformer Block × 𝐿𝐿

Embedding Layer

4

𝑡𝑡 2 𝑡𝑡 3 𝑡𝑡 𝑡𝑡 1

𝑡𝑡 4 𝑡𝑡 5 𝑡𝑡 𝑡𝑡 3

MTP Module 1

(Next 2  Token Prediction)

Output Head

Transformer Block

Linear Projection concatenation

RMSNorm RMSNorm

Embedding Layer

5

𝑡𝑡 3 𝑡𝑡 4 𝑡𝑡 𝑡𝑡 2

𝑡𝑡 5 𝑡𝑡 6 𝑡𝑡 𝑡𝑡 4

MTP Module 2

(Next 3  Token Prediction)

Output Head

Transformer Block

Linear Projection concatenation

RMSNorm RMSNorm

Embedding Layer

6

𝑡𝑡 4 𝑡𝑡 5 𝑡𝑡 𝑡𝑡 3

5

ℒ

𝑀𝑀𝑀𝑀𝑀𝑀𝑀𝑀

Shared

Shared

6

ℒ

1

MTP

Shared

Shared

7

ℒ

2

MTP

···
