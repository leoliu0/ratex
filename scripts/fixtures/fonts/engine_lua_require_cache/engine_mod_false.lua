-- Returning false is a non-nil result, but a false package.loaded entry is not "loaded".
engine_false_loads = (engine_false_loads or 0) + 1
return false
