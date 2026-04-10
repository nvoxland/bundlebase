import bundlebase.sync as bb

bundle = bb.open("random-data")

print(bundle.query(sql="select * from bundle where id > 30"))
