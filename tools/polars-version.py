#!/usr/bin/env python3
"""Print the Polars version, for the benchmark report.

The comparison records the version of every incumbent, because a number
measured against Polars 1.9 is not the same claim as one measured against
Polars 1.36. The Parquet path is accepted and ignored: the argument exists so
that this script has the same shape as `polars-run.py`.
"""

import sys

import polars as pl

print(f"polars {pl.__version__}")
