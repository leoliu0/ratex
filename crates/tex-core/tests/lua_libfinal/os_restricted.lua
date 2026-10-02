P("none", os.execute())
P("true", os.execute("true"))
P("empty", os.execute(""))
P("spawnno", os.spawn({"true"}))
P("popenno", io.popen("true"))

