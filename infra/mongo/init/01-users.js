// Runs once when the MongoDB data volume is initialised.
// Two databases, each with its own user (spec: Security).
const bankPwd = process.env.MONGO_BANK_PASSWORD;
const genPwd = process.env.MONGO_GENERATOR_PASSWORD;
if (!bankPwd || !genPwd) { throw new Error("MONGO_BANK_PASSWORD and MONGO_GENERATOR_PASSWORD must be set"); }

db.getSiblingDB("bank_logs").createUser({ user: "bank", pwd: bankPwd, roles: [{ role: "readWrite", db: "bank_logs" }] });
db.getSiblingDB("truth").createUser({ user: "generator", pwd: genPwd, roles: [{ role: "readWrite", db: "truth" }] });
