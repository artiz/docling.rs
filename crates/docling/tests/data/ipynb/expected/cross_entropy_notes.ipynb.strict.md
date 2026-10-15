# Cross entropy in one page

![Shannon](https://upload.wikimedia.org/wikipedia/commons/9/99/ClaudeShannon_MFO3807.jpg)

The entropy of a distribution $y$ is the expected number of bits under the optimal code, with $\log \frac{1}{y_i}$ bits for the $i$-th symbol:

$$H(y) = \sum_i y_i \log \frac{1}{y_i} = -\sum_i y_i \log y_i$$

Encoding symbols from $y$ with the wrong tool $\hat{y}$ costs more; the cost is $0.10 per bit and $5 per message.

## KL divergence

$$\mbox{KL}(y~||~\hat{y})
= \sum_i y_i \log \frac{1}{\hat{y}_i} - \sum_i y_i \log \frac{1}{y_i}
= \sum_i y_i \log \frac{y_i}{\hat{y}_i}$$

It is never negative, and it is 0 only when $y = \hat{y}$ — see [$$E=mc^2$$](https://en.wikipedia.org/wiki/Mass%E2%80%93energy_equivalence) for a formula that stays inline.

```python
import numpy as np
y = np.array([1.0, 0.0, 0.0]); y_hat = np.array([0.4, 0.1, 0.5])
float(-(y * np.log(y_hat)).sum())
```
