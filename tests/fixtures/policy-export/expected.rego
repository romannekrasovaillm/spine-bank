# Сгенерировано `arch-be policy export rego` из секции `deployment:` CONSTRAINTS.yaml.
# Пакет для OPA/Conftest (Rego v1): объект проходит, если множество `deny` пусто.
package archbe.deployment

# Все контейнеры Pod: spec.containers + spec.initContainers.
containers := array.concat(spec_list("containers"), spec_list("initContainers"))

spec_list(key) := object.get(object.get(input, "spec", {}), key, [])

# Инвариант: образы только из внутреннего реестра.
deny contains msg if {
    some c in containers
    not startswith(c.image, "registry.example.com/bank/")
    msg := sprintf("образ %q вне разрешённого реестра %q", [c.image, "registry.example.com/bank/"])
}

# Инвариант: образы подписаны. Криптопроверка cosign — за Kyverno verifyImages;
# здесь удерживается следствие подписанной поставки — адресация по дайджесту.
deny contains msg if {
    some c in containers
    not regex.match("^[^@]+@sha256:[0-9a-f]{64}$", c.image)
    msg := sprintf("образ %q не закреплён по дайджесту (@sha256:)", [c.image])
}

# Инвариант: контейнеры работают от non-root.
deny contains msg if {
    some c in containers
    not pod_non_root
    not container_non_root(c)
    msg := sprintf("контейнер %q: securityContext.runAsNonRoot обязателен", [object.get(c, "name", "<без имени>")])
}

pod_non_root if {
    input.spec.securityContext.runAsNonRoot == true
}

container_non_root(c) if {
    c.securityContext.runAsNonRoot == true
}

# Инвариант: limits не заданы или превышают бюджет.
deny contains msg if {
    some c in containers
    not limits_within_budget(c)
    msg := sprintf("контейнер %q: limits не заданы или превышают бюджет (cpu <= 500m, memory <= 512Mi)", [object.get(c, "name", "<без имени>")])
}

limits_within_budget(c) if {
    units.parse(sprintf("%v", [c.resources.limits.cpu])) <= units.parse("500m")
    units.parse_bytes(sprintf("%v", [c.resources.limits.memory])) <= units.parse_bytes("512Mi")
}

# Инвариант: deny-список образов.
deny_images := [
    "docker.io/library/nginx",
    "docker.io/library/redis",
]

deny contains msg if {
    some c in containers
    image_denied(c.image)
    msg := sprintf("образ %q в deny-списке развёртывания", [c.image])
}

image_denied(image) if {
    some d in deny_images
    image == d
}

image_denied(image) if {
    some d in deny_images
    startswith(image, sprintf("%s:", [d]))
}

image_denied(image) if {
    some d in deny_images
    startswith(image, sprintf("%s@", [d]))
}

# allow — вердикт по объекту: нарушений нет.
allow if {
    count(deny) == 0
}
