#! /home/nano/.cargo/bin/nu

mkdir data;
(
	docker run
	-d
	--rm
	--name pg
	-e POSTGRES_USER=forecast
	-e POSTGRES_PASSWORD=forecast
	-e PGDATA=/var/lib/postgresql/data
	-v ./data:/var/lib/postgresql/data
	-v /etc/passwd:/etc/passwd:ro
	--user $'(id -u):(id -g)'
	-p 5432:5432
	postgres
)
