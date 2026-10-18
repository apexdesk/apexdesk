' corpus-source: generated
' corpus-flagged: no
' corpus-prints: ready
'--- module Greeter class
Public State As String
Private Sub Class_Initialize()
    State = "ready"
End Sub
'--- module Module1 standard
Sub Main()
    Dim g As New Greeter
    Debug.Print g.State
End Sub
