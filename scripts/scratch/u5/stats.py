import json, sys
for f in sys.argv[1:]:
    d = json.load(open(f))["stats"]
    print(f, "hits", d["cache_hits"]["counts"], "misses", d["cache_misses"]["counts"],
          "writes", d["cache_writes"], "write_err", d["cache_write_errors"],
          "read_err", d["cache_read_errors"], "cache_err", d["cache_errors"]["counts"],
          "timeouts", d["cache_timeouts"], "compile_fails", d["compile_fails"])
