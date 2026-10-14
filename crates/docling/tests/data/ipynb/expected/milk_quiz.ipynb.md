```
%matplotlib inline

import random
import pandas as pd
import matplotlib.pyplot as plt
from sklearn.preprocessing import normalize
import numpy as np
```

```
data = pd.read_csv('monthly-milk-production.csv', sep = ';')
data.milk = data.milk.apply(pd.to_numeric)
data['month_dt'] = pd.to_datetime(data.month, dayfirst = True)
data['milk_daily'] = np.array([float(m) for m in data['milk']])/[m.days_in_month for m in data['month_dt']]

data.head(6)
```

```
# 6.
pl = data['milk_daily'].plot()
```

```
# 7.
import statsmodels.api as sm
adf_pvalue = sm.tsa.stattools.adfuller(data['milk_daily'].values)[0]
D = [(0, adf_pvalue)]
d = [(0, adf_pvalue)]

data_len = len(data['milk_daily'])
for ndx in range(1, 12): 
    diff = data['milk_daily'] - data['milk_daily'].shift(ndx * 12)
    diff.dropna(inplace = True)
    # print(len(diff.values), len(data['milk_daily'].values))
    adf = sm.tsa.stattools.adfuller(diff.values)
    # print(adf)
    D.append((ndx, adf[1]))
    
for ndx in range(1, data_len // 2): 
    diff = data['milk_daily'] - data['milk_daily'].shift(ndx)
    diff.dropna(inplace = True)
    adf = sm.tsa.stattools.adfuller(diff.values)
    d.append((ndx, adf[1]))
    

print('D', D[:5])
print('d', d[:5])

print('D min', sorted(D, key = lambda _: _[1])[:5])
print('d min', sorted(d, key = lambda _: _[1])[:5])

# Ряд становится стационарным после однократного дифференцирования - NO
# Ряд становится стационарным после сезонного дифференцирования - NO
# Ряд становится стационарным после двух последовательных дифференцирований — сезонного и обычного - YES !!!!

diff = data['milk_daily'] - data['milk_daily'].shift(12)
diff.dropna(inplace = True)

print(len(diff.values))

diff = diff - diff.shift(1)
diff.dropna(inplace = True)

print(len(diff.values))
adf = sm.tsa.stattools.adfuller(diff.values)
print(adf)
```

```
data['milk_shifted'] = diff # - data['milk_daily'].shift(12)
pl = data['milk_shifted'].plot()
```

```
# 8.
import statsmodels.api as sm
pl = sm.graphics.tsa.plot_acf(data['milk_daily'].values.squeeze(), lags=100)
```

```
###### import statsmodels.api as sm
plt.figure(figsize = (20,20))
pl = sm.graphics.tsa.plot_acf(data.milk_shifted.dropna().squeeze(), lags=50)
# 8.
# Q = 4 (48) - NO
# Q = 0 - NO
# Q = 1 - YES !!!!
```

```
# 9.
import statsmodels.api as sm
p = sm.graphics.tsa.plot_pacf(data.milk_shifted.dropna().squeeze(), lags=50)

# p = 13 - NO
# p = 25 - NO
# p = 12 - NO

# p = 0 - ?
```
