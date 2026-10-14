# Задание по программированию: Рекомендательные системы

Библиотеки

```
import pandas as pd
import numpy as np

%pylab inline
import pandas as pd
from scipy import stats
import statsmodels
import statsmodels.api as sm
import matplotlib.pyplot as plt
import warnings
import itertools

warnings.simplefilter('ignore')

# fix cyrillic fonts
from matplotlib import rc
font = {'family': 'Verdana','weight': 'normal'}
rc('font', **font)
```

Считаем данные

```
def read_sessions(file_name):
    sessions = []
    with open(file_name, 'r') as f:
        for l in f.readlines():
            parts = str.split(l.strip(), ';')
            sessions.append(([int(_) for _ in str.split(parts[0], ',')], 
                      [int(_) for _ in str.split(parts[1], ',')] if len(parts[1]) > 0 else []))
    return sessions


train = read_sessions ('coursera_sessions_train.txt')
test = read_sessions ('coursera_sessions_test.txt')
```

```
train[:5]
```

```
test[:5]
```

```
from collections import Counter, defaultdict

def calc_model(sessions):
    views = defaultdict(int)
    purchases = defaultdict(int)
    for sess in sessions:
        for k, v in Counter(sess[0]).items():
            views[k] += v
        if len(sess[1]):
            for k, v in Counter(sess[1]).items():
                purchases[k] += v

    return views, purchases
```

```
train_views, train_purchases = calc_model(train)
test_views, test_purchases = calc_model(test)
```

```
train_purchases
```

```
train_views_list = np.array(list([k,v] for k,v in train_views.items()))
train_purchases_list = np.array(list([k,v] for k,v in train_purchases.items()))

train_views_sorted = train_views_list[train_views_list[:,1].argsort(kind = 'mergesort')][::-1]
train_purchases_sorted = train_purchases_list[train_purchases_list[:,1].argsort(kind = 'mergesort')][::-1]

print(train_views_sorted[:5], '\n\n', train_views_sorted[-5:])
print(train_purchases_sorted[:5], '\n\n', train_purchases_sorted[-5:])
```

```
import operator
def recommend_by(sessions, recommendations_model):
    #   теперь надо показывать не последние просмотренные пользователем товары, 
    # а те товары из просмотренных, которые он наиболее вероятно купит.
    metrics = np.zeros(4) 
    count = 0
    for sess in sessions:
        # skip empty sessions
        if not sess[1]:
            continue

        count += 1
        views = pd.unique(sess[0])
        purchases = sess[1]
        
        popularities = [recommendations_model[id] for id in views]
        
        # works for 1 and 2
        # recommended = views[np.argsort(popularities, kind = 'mergesort')][::-1]
        
        popularities_2 = [(id, recommendations_model[id]) for id in views]
        recommended = list(map(operator.itemgetter(0), 
                               sorted(popularities_2, key=operator.itemgetter(1), reverse=True)))
        
        # print(purchases, recommended)
        # break
        
        # AverageRecall@1, AveragePrecision@1, AverageRecall@5, AveragePrecision@5
        precision1 = 1. if recommended[0] in purchases else 0.
        recall1 = 1./len(purchases) if recommended[0] in purchases else 0.
        
        recommended_count = min(5, len(views))
        purchases_intercept = len(np.intersect1d(purchases, recommended[:5]))
        
        precision5 = purchases_intercept / 5.
        recall5 = purchases_intercept / len(purchases)
                
        metrics[0] += recall1
        metrics[1] += precision1
        metrics[2] += recall5
        metrics[3] += precision5
        
    return np.round(metrics / count, 2)

def write_answer(filename, metrics):
    metrics_str = ' '.join(([str(round(_, 2)) for _ in metrics]))
    print(filename, ':', metrics_str)
    with open(filename, 'w') as f:
        f.write(metrics_str)
```

```
write_answer('recommend_by_views_train.txt', recommend_by(train, train_views))
```

```
write_answer('recommend_by_views_test.txt', recommend_by(test, train_views))
```

```
res = recommend_by(train, train_purchases)    
invalid=[0.66, 0.78, 0.92, 0.25]
print(res == invalid)
write_answer('recommend_by_purchases_train.txt', recommend_by(train, train_purchases))
```

```
res = recommend_by(test, train_purchases)    
invalid=[0.4, 0.46, 0.76, 0.2]
print(res == invalid)
write_answer('recommend_by_purchases_test.txt', res)
```

```
# test
views_t = ['59', '60', '61', '62', '60', '63', '64', '65', '66', '61', '67', '68', '67']
purchases_t = ['67','60', '63']
recommend_by([[views_t, purchases_t]], Counter(views_t))
```
